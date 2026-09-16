//! Model execution scheduling is independent of platform transport and tools.
use crate::{ai::{AiProvider,AiRequest}, conversations::ConversationKey, error::{AppError,AppResult}, models::AiDecision, shutdown::ShutdownSignal};
use std::{collections::HashMap,sync::{Arc,Mutex,Weak},time::Duration};
use tokio::sync::{Mutex as AsyncMutex,Semaphore};

pub struct ModelExecutor {
    sessions: Mutex<HashMap<ConversationKey,Weak<AsyncMutex<()>>>>,
    concurrency: Semaphore,
    deadline: Duration,
}
impl Default for ModelExecutor {
    fn default() -> Self { Self { sessions:Mutex::new(HashMap::new()),concurrency:Semaphore::new(4),deadline:Duration::from_secs(45) } }
}
impl ModelExecutor {
    pub async fn decide(&self, key: ConversationKey, provider: &dyn AiProvider, request: &AiRequest, shutdown: &ShutdownSignal) -> AppResult<AiDecision> {
        let session = {
            let mut sessions = self.sessions.lock().map_err(|_| AppError::new("ai_scheduler","AI 调度不可用"))?;
            sessions.retain(|_,lock| lock.strong_count()>0);
            match sessions.get(&key).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => { let lock=Arc::new(AsyncMutex::new(())); sessions.insert(key,Arc::downgrade(&lock)); lock }
            }
        };
        // Queue wait is included in the budget; cancellation drops the model future.
        tokio::select! {
            biased;
            _=shutdown.cancelled() => Err(AppError::new("ai_cancelled","AI 处理已取消")),
            result=tokio::time::timeout(self.deadline,async {
                let _session=session.lock().await;
                let _permit=self.concurrency.acquire().await.map_err(|_| AppError::new("ai_scheduler","AI 调度已停止"))?;
                provider.decide(request).await
            }) => result.map_err(|_| AppError::new("ai_timeout","AI 处理超时").retryable())?,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize,Ordering};
    struct Probe { active:AtomicUsize, peak:AtomicUsize }
    #[async_trait]
    impl AiProvider for Probe {
        async fn decide(&self,_:&AiRequest)->AppResult<AiDecision> {
            let active=self.active.fetch_add(1,Ordering::SeqCst)+1;
            self.peak.fetch_max(active,Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
            self.active.fetch_sub(1,Ordering::SeqCst);
            Err(AppError::new("probe","probe"))
        }
    }
    fn key(kind:&str)->ConversationKey { ConversationKey {platform:"wangshangliao".into(),account_id:"a".into(),kind:kind.into(),target:"same".into()} }
    #[tokio::test]
    async fn serializes_same_conversation_but_allows_private_and_group_concurrency() {
        let executor=ModelExecutor::default();let shutdown=ShutdownSignal::default();let request=AiRequest::testing("test",vec![]);
        let provider=Probe {active:AtomicUsize::new(0),peak:AtomicUsize::new(0)};
        let _=tokio::join!(executor.decide(key("group"),&provider,&request,&shutdown),executor.decide(key("group"),&provider,&request,&shutdown));
        assert_eq!(provider.peak.load(Ordering::SeqCst),1);
        let _=tokio::join!(executor.decide(key("group"),&provider,&request,&shutdown),executor.decide(key("private"),&provider,&request,&shutdown));
        assert_eq!(provider.peak.load(Ordering::SeqCst),2);
        shutdown.cancel();
        assert_eq!(executor.decide(key("group"),&provider,&request,&shutdown).await.unwrap_err().code,"ai_cancelled");
    }
}
