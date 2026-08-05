use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;

use crate::diagnostics::redact;

const SUCCESS_RETENTION: Duration = Duration::from_secs(5);
const MAX_TRACKED_ITEMS: usize = 200;
const EVENT_COALESCE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeWorkItem {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub scope_label: String,
    pub state: String,
    pub percent: Option<u8>,
    pub queued: usize,
    pub started_at: Option<DateTime<Utc>>,
    pub retry_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeWorkCounts {
    pub running: usize,
    pub queued: usize,
    pub retrying: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeWorkSnapshot {
    pub active: Option<RuntimeWorkItem>,
    pub items: Vec<RuntimeWorkItem>,
    pub counts: RuntimeWorkCounts,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct TrackedWork {
    id: String,
    kind: String,
    label: String,
    scope_label: String,
    state: String,
    queued: usize,
    current_step: Option<usize>,
    total_steps: Option<usize>,
    started_at: Option<DateTime<Utc>>,
    retry_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    error: String,
    sequence: u64,
}

impl TrackedWork {
    fn public(&self) -> RuntimeWorkItem {
        let percent = match (self.current_step, self.total_steps) {
            (Some(current), Some(total)) if total > 0 => {
                Some(((current.min(total) * 100) / total).min(100) as u8)
            }
            _ => None,
        };
        RuntimeWorkItem {
            id: self.id.clone(),
            kind: self.kind.clone(),
            label: self.label.clone(),
            scope_label: self.scope_label.clone(),
            state: self.state.clone(),
            percent,
            queued: self.queued,
            started_at: self.started_at,
            retry_at: self.retry_at,
            completed_at: self.completed_at,
            error: self.error.clone(),
        }
    }

    fn terminal(&self) -> bool {
        matches!(self.state.as_str(), "succeeded" | "failed" | "unknown")
    }
}

#[derive(Default)]
struct WorkState {
    items: HashMap<String, TrackedWork>,
    pinned_active: Option<String>,
    next_sequence: u64,
    updated_at: Option<DateTime<Utc>>,
}

struct TrackerInner {
    state: Mutex<WorkState>,
    app: Mutex<Option<AppHandle>>,
    emit_pending: AtomicBool,
}

#[derive(Clone)]
pub struct RuntimeWorkTracker {
    inner: Arc<TrackerInner>,
}

impl Default for RuntimeWorkTracker {
    fn default() -> Self {
        Self {
            inner: Arc::new(TrackerInner {
                state: Mutex::new(WorkState::default()),
                app: Mutex::new(None),
                emit_pending: AtomicBool::new(false),
            }),
        }
    }
}

impl RuntimeWorkTracker {
    pub fn attach(&self, app: AppHandle) {
        if let Ok(mut current) = self.inner.app.lock() {
            *current = Some(app);
        }
        self.changed();
    }

    pub fn enqueue(&self, id: &str, kind: &str, label: &str, scope_label: &str) {
        self.enqueue_count(id, kind, label, scope_label, 1);
    }

    pub fn enqueue_count(
        &self,
        id: &str,
        kind: &str,
        label: &str,
        scope_label: &str,
        count: usize,
    ) {
        if count == 0 {
            return;
        }
        self.mutate(|state| {
            let sequence = state.next_sequence;
            state.next_sequence = state.next_sequence.saturating_add(1);
            let item = state
                .items
                .entry(id.to_string())
                .or_insert_with(|| TrackedWork {
                    id: id.to_string(),
                    kind: kind.to_string(),
                    label: label.to_string(),
                    scope_label: scope_label.to_string(),
                    state: "queued".into(),
                    queued: 0,
                    current_step: None,
                    total_steps: None,
                    started_at: None,
                    retry_at: None,
                    completed_at: None,
                    error: String::new(),
                    sequence,
                });
            if item.terminal() {
                item.state = "queued".into();
                item.completed_at = None;
                item.error.clear();
                item.retry_at = None;
            }
            item.kind = kind.to_string();
            item.label = label.to_string();
            item.scope_label = scope_label.to_string();
            item.queued = item.queued.saturating_add(count);
        });
    }

    pub fn seed_queued_count(
        &self,
        id: &str,
        kind: &str,
        label: &str,
        scope_label: &str,
        count: usize,
    ) {
        if count == 0 {
            return;
        }
        self.mutate(|state| {
            let sequence = state.next_sequence;
            state.next_sequence = state.next_sequence.saturating_add(1);
            let item = state
                .items
                .entry(id.to_string())
                .or_insert_with(|| TrackedWork {
                    id: id.to_string(),
                    kind: kind.to_string(),
                    label: label.to_string(),
                    scope_label: scope_label.to_string(),
                    state: "queued".into(),
                    queued: 0,
                    current_step: None,
                    total_steps: None,
                    started_at: None,
                    retry_at: None,
                    completed_at: None,
                    error: String::new(),
                    sequence,
                });
            if item.terminal() {
                item.state = "queued".into();
                item.completed_at = None;
                item.error.clear();
                item.retry_at = None;
            }
            item.kind = kind.to_string();
            item.label = label.to_string();
            item.scope_label = scope_label.to_string();
            let database_queued = if item.state == "running" {
                count.saturating_sub(1)
            } else {
                count
            };
            item.queued = item.queued.max(database_queued);
        });
    }

    pub fn start(&self, id: &str, total_steps: Option<usize>) {
        self.mutate(|state| {
            let Some(item) = state.items.get_mut(id) else {
                return;
            };
            item.queued = item.queued.saturating_sub(1);
            item.state = "running".into();
            item.started_at = Some(Utc::now());
            item.completed_at = None;
            item.current_step = total_steps.map(|_| 0);
            item.total_steps = total_steps;
            item.retry_at = None;
            item.error.clear();
            if state.pinned_active.is_none() {
                state.pinned_active = Some(id.to_string());
            }
        });
    }

    pub fn start_named(
        &self,
        id: &str,
        kind: &str,
        label: &str,
        scope_label: &str,
        total_steps: Option<usize>,
    ) {
        let exists = self
            .inner
            .state
            .lock()
            .map(|state| state.items.contains_key(id))
            .unwrap_or(false);
        if !exists {
            self.enqueue(id, kind, label, scope_label);
        }
        self.start(id, total_steps);
    }

    pub fn reject_enqueue(&self, id: &str, error: &str) {
        self.mutate(|state| {
            let Some(item) = state.items.get_mut(id) else {
                return;
            };
            item.queued = item.queued.saturating_sub(1);
            if item.queued == 0 && item.state != "running" {
                item.state = "failed".into();
                item.completed_at = Some(Utc::now());
                item.error = sanitize_text(error);
            }
        });
    }

    pub fn progress(&self, id: &str, completed: usize, total: usize) {
        self.mutate(|state| {
            let Some(item) = state.items.get_mut(id) else {
                return;
            };
            item.current_step = Some(completed.min(total));
            item.total_steps = Some(total.max(1));
        });
    }

    pub fn retrying(&self, id: &str, retry_at: Option<DateTime<Utc>>, error: &str) {
        self.mutate(|state| {
            let Some(item) = state.items.get_mut(id) else {
                return;
            };
            item.state = "retrying".into();
            item.retry_at = retry_at;
            item.error = sanitize_text(error);
            item.started_at = None;
            item.current_step = None;
            item.total_steps = None;
            item.queued = item.queued.max(1);
            if state.pinned_active.as_deref() == Some(id) {
                state.pinned_active = None;
            }
        });
    }

    pub fn finish(&self, id: &str, outcome: &str, error: &str) {
        self.mutate(|state| {
            let Some(item) = state.items.get_mut(id) else {
                return;
            };
            if item.queued > 0 && outcome == "succeeded" {
                item.state = "queued".into();
                item.started_at = None;
                item.current_step = None;
                item.total_steps = None;
            } else {
                item.state = match outcome {
                    "failed" => "failed",
                    "unknown" => "unknown",
                    _ => "succeeded",
                }
                .into();
                item.completed_at = Some(Utc::now());
                item.current_step = item.total_steps;
                item.error = sanitize_text(error);
            }
            if state.pinned_active.as_deref() == Some(id) {
                state.pinned_active = None;
            }
        });
        if outcome == "succeeded" && self.has_app() {
            let tracker = self.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(SUCCESS_RETENTION).await;
                tracker.changed();
            });
        }
    }

    pub fn acknowledge_failures(&self, ids: &[String]) {
        self.mutate(|state| {
            state.items.retain(|id, item| {
                !ids.iter().any(|candidate| candidate == id)
                    || !matches!(item.state.as_str(), "failed" | "unknown")
            });
        });
    }

    pub fn snapshot(&self) -> RuntimeWorkSnapshot {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        prune(&mut state);
        choose_active(&mut state);
        snapshot_from_state(&state)
    }

    fn mutate(&self, update: impl FnOnce(&mut WorkState)) {
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            prune(&mut state);
            update(&mut state);
            enforce_bound(&mut state);
            choose_active(&mut state);
            state.updated_at = Some(Utc::now());
        }
        self.changed();
    }

    fn has_app(&self) -> bool {
        self.inner
            .app
            .lock()
            .map(|value| value.is_some())
            .unwrap_or(false)
    }

    fn changed(&self) {
        if !self.has_app() || self.inner.emit_pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let tracker = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(EVENT_COALESCE).await;
            tracker.inner.emit_pending.store(false, Ordering::Release);
            let snapshot = tracker.snapshot();
            if let Ok(app) = tracker.inner.app.lock() {
                if let Some(app) = app.as_ref() {
                    let _ = app.emit("runtime-work-updated", snapshot);
                }
            }
        });
    }
}

#[derive(Clone, Default)]
pub struct RuntimeCoordination {
    pub tracker: RuntimeWorkTracker,
    pub wake: Arc<Notify>,
}

impl RuntimeCoordination {
    pub fn notify(&self) {
        self.wake.notify_waiters();
        self.wake.notify_one();
    }
}

fn sanitize_text(value: &str) -> String {
    redact(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

fn priority(kind: &str) -> u8 {
    match kind {
        "aiRule" => 0,
        "message" | "write" | "cardRename" => 1,
        "aiReply" => 2,
        "sync" | "schedule" | "reminder" => 3,
        _ => 4,
    }
}

fn choose_active(state: &mut WorkState) {
    if state
        .pinned_active
        .as_ref()
        .and_then(|id| state.items.get(id))
        .is_some_and(|item| item.state == "running")
    {
        return;
    }
    state.pinned_active = state
        .items
        .values()
        .filter(|item| item.state == "running")
        .min_by_key(|item| (priority(&item.kind), item.sequence))
        .map(|item| item.id.clone());
}

fn prune(state: &mut WorkState) {
    let now = Utc::now();
    state.items.retain(|_, item| {
        if item.state != "succeeded" {
            return true;
        }
        item.completed_at.is_some_and(|completed| {
            now.signed_duration_since(completed)
                .to_std()
                .unwrap_or_default()
                < SUCCESS_RETENTION
        })
    });
}

fn enforce_bound(state: &mut WorkState) {
    if state.items.len() <= MAX_TRACKED_ITEMS {
        return;
    }
    let mut removable = state
        .items
        .values()
        .filter(|item| item.terminal())
        .map(|item| (item.sequence, item.id.clone()))
        .collect::<Vec<_>>();
    removable.sort_by_key(|(sequence, _)| *sequence);
    for (_, id) in removable {
        if state.items.len() <= MAX_TRACKED_ITEMS {
            break;
        }
        state.items.remove(&id);
    }
}

fn snapshot_from_state(state: &WorkState) -> RuntimeWorkSnapshot {
    let mut items = state
        .items
        .values()
        .map(TrackedWork::public)
        .collect::<Vec<_>>();
    items.sort_by_key(|item| {
        let state_order = match item.state.as_str() {
            "running" => 0,
            "queued" => 1,
            "retrying" => 2,
            "failed" | "unknown" => 3,
            _ => 4,
        };
        (state_order, priority(&item.kind), item.started_at)
    });
    let counts = RuntimeWorkCounts {
        running: items.iter().filter(|item| item.state == "running").count(),
        queued: items.iter().map(|item| item.queued).sum(),
        retrying: items.iter().filter(|item| item.state == "retrying").count(),
        failed: items
            .iter()
            .filter(|item| matches!(item.state.as_str(), "failed" | "unknown"))
            .count(),
    };
    let active = state
        .pinned_active
        .as_ref()
        .and_then(|id| state.items.get(id))
        .map(TrackedWork::public);
    RuntimeWorkSnapshot {
        active,
        items,
        counts,
        updated_at: state.updated_at.unwrap_or_else(Utc::now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_lifecycle_reports_progress_and_queue_depth() {
        let tracker = RuntimeWorkTracker::default();
        tracker.enqueue("messages-a", "message", "处理群消息", "测试群");
        tracker.enqueue("messages-a", "message", "处理群消息", "测试群");
        tracker.start("messages-a", Some(4));
        tracker.progress("messages-a", 2, 4);
        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.active.unwrap().percent, Some(50));
        assert_eq!(snapshot.counts.running, 1);
        assert_eq!(snapshot.counts.queued, 1);
        tracker.finish("messages-a", "succeeded", "");
        assert_eq!(tracker.snapshot().items[0].state, "queued");
    }

    #[test]
    fn failures_are_retained_until_acknowledged() {
        let tracker = RuntimeWorkTracker::default();
        tracker.enqueue("write-a", "write", "执行群操作", "测试群");
        tracker.start("write-a", None);
        tracker.finish("write-a", "unknown", "unknown receipt");
        assert_eq!(tracker.snapshot().counts.failed, 1);
        tracker.acknowledge_failures(&["write-a".into()]);
        assert!(tracker.snapshot().items.is_empty());
    }

    #[test]
    fn acknowledgement_only_removes_failures_visible_to_the_caller() {
        let tracker = RuntimeWorkTracker::default();
        for id in ["write-a", "write-b"] {
            tracker.enqueue(id, "write", "发送群消息", "群操作");
            tracker.start(id, None);
            tracker.finish(id, "failed", "failed receipt");
        }
        tracker.acknowledge_failures(&["write-a".into()]);
        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.counts.failed, 1);
        assert_eq!(snapshot.items[0].id, "write-b");
    }

    #[test]
    fn active_task_stays_pinned_until_it_finishes() {
        let tracker = RuntimeWorkTracker::default();
        tracker.enqueue("prediction", "prediction", "生成预测说明", "");
        tracker.start("prediction", None);
        tracker.enqueue("rule", "aiRule", "AI 规则判断", "测试群");
        tracker.start("rule", None);
        assert_eq!(tracker.snapshot().active.unwrap().id, "prediction");
        tracker.finish("prediction", "succeeded", "");
        assert_eq!(tracker.snapshot().active.unwrap().id, "rule");
    }

    #[test]
    fn startup_backlog_does_not_double_count_already_observed_work() {
        let tracker = RuntimeWorkTracker::default();
        tracker.enqueue_count("messages-a", "message", "处理群消息", "群消息队列", 2);
        tracker.seed_queued_count("messages-a", "message", "处理群消息", "群消息队列", 3);
        tracker.seed_queued_count("messages-a", "message", "处理群消息", "群消息队列", 3);
        assert_eq!(tracker.snapshot().counts.queued, 3);
    }

    #[test]
    fn startup_backlog_excludes_the_running_database_item() {
        let tracker = RuntimeWorkTracker::default();
        tracker.enqueue("messages-a", "message", "处理群消息", "群消息队列");
        tracker.start("messages-a", Some(4));
        tracker.seed_queued_count("messages-a", "message", "处理群消息", "群消息队列", 3);
        assert_eq!(tracker.snapshot().counts.running, 1);
        assert_eq!(tracker.snapshot().counts.queued, 2);
        tracker.finish("messages-a", "succeeded", "");
        assert_eq!(tracker.snapshot().counts.queued, 2);
    }
}
