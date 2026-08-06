//! Queue kernel: middleware-based effect dispatch pipeline.
//!
//! Phase 2 infrastructure. The old effect_loop remains intact until Phase 3
//! wires this module in as the live Dispatcher. All public items here are
//! intentionally unused at the crate level until that phase.
//!
//! Architecture:
//! - `DispatchContext`: carries the item + metadata through the chain
//! - `DispatchDecision`: what the middleware decided (Continue/Skip/Retry/Fail)
//! - `EffectMiddleware`: trait for chainable pre-dispatch logic
//! - `MiddlewareChain`: executes middleware in order
//! - Built-in middleware: ExpiryGuard, OrderKeyLock, LaneConcurrencyGate
//! - `QueueKernel`: top-level coordinator (chain + per-lane gate + order-key lock)

// Phase 2: infrastructure only. Phase 3 will consume these types from runtime.rs.
#![allow(dead_code)]

use crate::error::AppError;
use crate::models::EffectOutboxItem;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, Semaphore};

/// Metadata attached to a dispatch context.
#[derive(Debug, Clone)]
pub struct DispatchMetadata {
    pub claimed_at: DateTime<Utc>,
    pub correlation_id: String,
    pub origin: String,
}

/// Context passed through the middleware chain.
#[derive(Debug, Clone)]
pub struct DispatchContext {
    pub item: EffectOutboxItem,
    pub account_id: String,
    pub metadata: DispatchMetadata,
}

/// Middleware decision: how to proceed with this item.
#[derive(Debug, Clone)]
pub enum DispatchDecision {
    /// Continue to next middleware.
    Continue,
    /// Skip this item (mark as succeeded without dispatching).
    Skip { reason: String },
    /// Permanent failure (no retry).
    Fail { error: AppError },
    /// Transient failure (schedule retry with backoff).
    Retry { error: AppError },
}

/// Middleware hook: executed before dispatching an effect.
#[async_trait]
pub trait EffectMiddleware: Send + Sync {
    async fn before_dispatch(&self, ctx: &DispatchContext) -> DispatchDecision;
}

/// Ordered chain of middleware.
pub struct MiddlewareChain {
    middlewares: Vec<Arc<dyn EffectMiddleware>>,
}

impl MiddlewareChain {
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    pub fn with(mut self, middleware: Arc<dyn EffectMiddleware>) -> Self {
        self.middlewares.push(middleware);
        self
    }

    /// Execute the chain. Returns the first non-Continue decision.
    pub async fn execute(&self, ctx: &DispatchContext) -> DispatchDecision {
        for middleware in &self.middlewares {
            match middleware.before_dispatch(ctx).await {
                DispatchDecision::Continue => continue,
                decision => return decision,
            }
        }
        DispatchDecision::Continue
    }
}

impl Default for MiddlewareChain {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Built-in middleware
// ---------------------------------------------------------------------------

/// Skips any item whose `expires_at` is in the past.
pub struct ExpiryGuard {
    clock: Arc<dyn ClockSource>,
}

impl ExpiryGuard {
    pub fn new(clock: Arc<dyn ClockSource>) -> Self {
        Self { clock }
    }
}

#[async_trait]
impl EffectMiddleware for ExpiryGuard {
    async fn before_dispatch(&self, ctx: &DispatchContext) -> DispatchDecision {
        if let Some(expires_at) = ctx.item.expires_at {
            if expires_at <= self.clock.now() {
                return DispatchDecision::Skip {
                    reason: format!(
                        "item {} expired at {}",
                        ctx.item.id, expires_at
                    ),
                };
            }
        }
        DispatchDecision::Continue
    }
}

/// Serializes concurrent dispatch for items that share the same `order_key`.
///
/// At most one item per order_key executes at a time. The RAII guard
/// (`OrderKeyGuard`) must be held for the duration of the dispatch call.
pub struct OrderKeyLock {
    locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
}

impl OrderKeyLock {
    pub fn new() -> Self {
        Self {
            locks: RwLock::new(HashMap::new()),
        }
    }

    async fn lock_for(&self, key: &str) -> Arc<Mutex<()>> {
        // Fast path: lock already exists
        {
            let map = self.locks.read().await;
            if let Some(m) = map.get(key) {
                return m.clone();
            }
        }
        // Slow path: insert new lock
        let mut map = self.locks.write().await;
        map.entry(key.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

impl Default for OrderKeyLock {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard returned by `OrderKeyLock::acquire`. Dropping it releases the slot.
#[derive(Debug)]
pub struct OrderKeyGuard {
    // Holds the mutex guard for the duration of the dispatch.
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl OrderKeyLock {
    /// Acquire the order-key slot. Returns `None` if no order_key is set
    /// (no serialization needed).
    pub async fn acquire(&self, ctx: &DispatchContext) -> Option<OrderKeyGuard> {
        let key = ctx.item.order_key.as_deref()?;
        let mutex = self.lock_for(key).await;
        let guard = mutex.lock_owned().await;
        Some(OrderKeyGuard { _guard: guard })
    }
}

/// Per-lane concurrency cap.
///
/// Each named lane has an independent `Semaphore`. Items without a lane name
/// (empty string / "default") share the "default" semaphore.
pub struct LaneConcurrencyGate {
    default_permits: usize,
    gates: RwLock<HashMap<String, Arc<Semaphore>>>,
}

impl LaneConcurrencyGate {
    /// `default_permits` controls how many items from the same lane may be
    /// dispatched concurrently (default: 4).
    pub fn new(default_permits: usize) -> Self {
        Self {
            default_permits,
            gates: RwLock::new(HashMap::new()),
        }
    }

    async fn semaphore_for(&self, lane: &str) -> Arc<Semaphore> {
        let lane = if lane.is_empty() { "default" } else { lane };
        {
            let map = self.gates.read().await;
            if let Some(s) = map.get(lane) {
                return s.clone();
            }
        }
        let mut map = self.gates.write().await;
        map.entry(lane.to_owned())
            .or_insert_with(|| Arc::new(Semaphore::new(self.default_permits)))
            .clone()
    }

    /// Acquire a concurrency permit for the item's lane. The returned permit
    /// must be held for the duration of the dispatch call.
    pub async fn acquire(
        &self,
        ctx: &DispatchContext,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, tokio::sync::AcquireError> {
        let sem = self.semaphore_for(&ctx.item.lane).await;
        sem.acquire_owned().await
    }
}

// ---------------------------------------------------------------------------
// Clock source abstraction (separate from runtime::SystemClock to keep this
// module dependency-free from the rest of the crate)
// ---------------------------------------------------------------------------

pub trait ClockSource: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct UtcClock;

impl ClockSource for UtcClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

// ---------------------------------------------------------------------------
// QueueKernel: top-level coordinator
// ---------------------------------------------------------------------------

/// Outcome returned by `QueueKernel::run_chain`.
#[derive(Debug)]
pub enum ChainOutcome {
    /// Proceed to actual dispatch.
    Proceed {
        order_guard: Option<OrderKeyGuard>,
        lane_permit: tokio::sync::OwnedSemaphorePermit,
    },
    /// Item should be skipped (mark succeeded, no dispatch).
    Skip { reason: String },
    /// Permanent failure.
    Fail { error: AppError },
    /// Transient failure, schedule retry.
    Retry { error: AppError },
}

/// Queue kernel: middleware chain + per-lane concurrency + order-key lock.
///
/// Call `run_chain` before dispatching an effect. It runs all middleware and
/// acquires the appropriate slots. Holding the returned `ChainOutcome::Proceed`
/// guards the slots until the dispatch completes.
pub struct QueueKernel {
    chain: MiddlewareChain,
    order_lock: Arc<OrderKeyLock>,
    lane_gate: Arc<LaneConcurrencyGate>,
}

impl QueueKernel {
    /// Build with default settings (4 concurrent per lane).
    pub fn new(clock: Arc<dyn ClockSource>) -> Self {
        let expiry = Arc::new(ExpiryGuard::new(clock));
        let chain = MiddlewareChain::new().with(expiry);
        Self {
            chain,
            order_lock: Arc::new(OrderKeyLock::new()),
            lane_gate: Arc::new(LaneConcurrencyGate::new(4)),
        }
    }

    pub fn with_chain(
        chain: MiddlewareChain,
        order_lock: Arc<OrderKeyLock>,
        lane_gate: Arc<LaneConcurrencyGate>,
    ) -> Self {
        Self {
            chain,
            order_lock,
            lane_gate,
        }
    }

    /// Run the middleware chain and acquire concurrency slots.
    ///
    /// Returns a `ChainOutcome` that the caller inspects before dispatching.
    /// If `Proceed`, the caller MUST hold the returned guards until dispatch
    /// completes so that slots are released promptly.
    pub async fn run_chain(&self, item: EffectOutboxItem, account_id: String) -> ChainOutcome {
        let metadata = DispatchMetadata {
            claimed_at: Utc::now(),
            correlation_id: item.correlation_id.clone(),
            origin: item.origin.clone(),
        };
        let ctx = DispatchContext {
            item,
            account_id,
            metadata,
        };

        match self.chain.execute(&ctx).await {
            DispatchDecision::Skip { reason } => return ChainOutcome::Skip { reason },
            DispatchDecision::Fail { error } => return ChainOutcome::Fail { error },
            DispatchDecision::Retry { error } => return ChainOutcome::Retry { error },
            DispatchDecision::Continue => {}
        }

        // Acquire lane concurrency permit
        let lane_permit = match self.lane_gate.acquire(&ctx).await {
            Ok(permit) => permit,
            Err(_) => {
                return ChainOutcome::Fail {
                    error: AppError::new(
                        "queue_kernel_lane_closed",
                        "lane semaphore closed during shutdown",
                    ),
                }
            }
        };

        // Acquire order-key serialization slot (may be None if no order_key)
        let order_guard = self.order_lock.acquire(&ctx).await;

        ChainOutcome::Proceed {
            order_guard,
            lane_permit,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EffectOutboxItem;

    fn make_item(id: i64, lane: &str, order_key: Option<&str>, expires_at: Option<DateTime<Utc>>) -> EffectOutboxItem {
        EffectOutboxItem {
            id,
            account_id: "acc1".into(),
            group_id: 1,
            effect_type: "send_text".into(),
            payload_json: "{}".into(),
            dedupe_key: format!("key-{id}"),
            state: "processing".into(),
            attempts: 0,
            next_attempt_at: None,
            last_error: String::new(),
            receipt_json: String::new(),
            created_at: Utc::now(),
            claimed_at: None,
            completed_at: None,
            priority: 100,
            lane: lane.to_owned(),
            order_key: order_key.map(str::to_owned),
            correlation_id: String::new(),
            origin: "test".into(),
            expires_at,
        }
    }

    struct FixedClock(DateTime<Utc>);
    impl ClockSource for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    #[tokio::test]
    async fn expiry_guard_skips_expired() {
        let now = Utc::now();
        let past = now - chrono::Duration::seconds(1);
        let item = make_item(1, "default", None, Some(past));
        let clock: Arc<dyn ClockSource> = Arc::new(FixedClock(now));
        let kernel = QueueKernel::new(clock);
        let outcome = kernel.run_chain(item, "acc1".into()).await;
        assert!(
            matches!(outcome, ChainOutcome::Skip { .. }),
            "expired item must be skipped"
        );
    }

    #[tokio::test]
    async fn non_expired_item_proceeds() {
        let now = Utc::now();
        let future = now + chrono::Duration::seconds(60);
        let item = make_item(2, "default", None, Some(future));
        let clock: Arc<dyn ClockSource> = Arc::new(FixedClock(now));
        let kernel = QueueKernel::new(clock);
        let outcome = kernel.run_chain(item, "acc1".into()).await;
        assert!(
            matches!(outcome, ChainOutcome::Proceed { .. }),
            "non-expired item must proceed"
        );
    }

    #[tokio::test]
    async fn no_expiry_always_proceeds() {
        let now = Utc::now();
        let item = make_item(3, "default", None, None);
        let clock: Arc<dyn ClockSource> = Arc::new(FixedClock(now));
        let kernel = QueueKernel::new(clock);
        let outcome = kernel.run_chain(item, "acc1".into()).await;
        assert!(
            matches!(outcome, ChainOutcome::Proceed { .. }),
            "item with no expiry must proceed"
        );
    }

    #[tokio::test]
    async fn order_key_lock_serializes() {
        let lock = Arc::new(OrderKeyLock::new());
        let item = make_item(4, "default", Some("grp-42"), None);
        let ctx = DispatchContext {
            item: item.clone(),
            account_id: "acc1".into(),
            metadata: DispatchMetadata {
                claimed_at: Utc::now(),
                correlation_id: String::new(),
                origin: "test".into(),
            },
        };
        let guard = lock.acquire(&ctx).await;
        assert!(guard.is_some(), "order key guard must be acquired");
    }

    #[tokio::test]
    async fn order_key_none_returns_no_guard() {
        let lock = Arc::new(OrderKeyLock::new());
        let item = make_item(5, "default", None, None);
        let ctx = DispatchContext {
            item,
            account_id: "acc1".into(),
            metadata: DispatchMetadata {
                claimed_at: Utc::now(),
                correlation_id: String::new(),
                origin: "test".into(),
            },
        };
        let guard = lock.acquire(&ctx).await;
        assert!(guard.is_none(), "no order key => no guard");
    }

    #[tokio::test]
    async fn lane_gate_issues_permit() {
        let gate = LaneConcurrencyGate::new(2);
        let item = make_item(6, "priority_lane", None, None);
        let ctx = DispatchContext {
            item,
            account_id: "acc1".into(),
            metadata: DispatchMetadata {
                claimed_at: Utc::now(),
                correlation_id: String::new(),
                origin: "test".into(),
            },
        };
        let permit = gate.acquire(&ctx).await;
        assert!(permit.is_ok(), "lane gate must issue a permit");
    }
}
