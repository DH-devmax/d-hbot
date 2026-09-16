//! One coordinator per account. No assumptions about provider credential format.
//! Refresh completion updates only its own field and cannot resurrect a logout.
use std::future::Future;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Secret(Zeroizing<String>);
impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
#[derive(Clone)]
pub struct Snapshot {
    pub generation: u64,
    pub revision: u64,
    pub business: Secret,
    pub nim: Secret,
}
struct State {
    generation: u64,
    revision: u64,
    current: Option<(Secret, Secret)>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum RefreshError<E> {
    LoggedOut,
    Stale,
    Provider(E),
}

pub struct CredentialCoordinator {
    state: Mutex<State>,
    business_refresh: Mutex<()>,
    nim_refresh: Mutex<()>,
}
impl Default for CredentialCoordinator {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                generation: 0,
                revision: 0,
                current: None,
            }),
            business_refresh: Mutex::new(()),
            nim_refresh: Mutex::new(()),
        }
    }
}
impl CredentialCoordinator {
    pub async fn login(&self, business: Secret, nim: Secret) {
        let mut s = self.state.lock().await;
        s.generation += 1;
        s.revision += 1;
        s.current = Some((business, nim));
    }
    pub async fn logout(&self) {
        let mut s = self.state.lock().await;
        s.generation += 1;
        s.revision += 1;
        s.current = None;
    }
    pub async fn snapshot(&self) -> Option<Snapshot> {
        let s = self.state.lock().await;
        s.current.as_ref().map(|(business, nim)| Snapshot {
            generation: s.generation,
            revision: s.revision,
            business: business.clone(),
            nim: nim.clone(),
        })
    }
    /// Verify again after NIM registration, before making the connection active.
    pub async fn is_current(&self, snapshot: &Snapshot) -> bool {
        let s = self.state.lock().await;
        s.current.is_some()
            && s.generation == snapshot.generation
            && s.revision == snapshot.revision
    }
    pub async fn refresh_nim<F, Fut, E>(
        &self,
        observed: &Snapshot,
        refresh: F,
    ) -> Result<Snapshot, RefreshError<E>>
    where
        F: FnOnce(Snapshot) -> Fut,
        Fut: Future<Output = Result<Secret, E>>,
    {
        let _single = self.nim_refresh.lock().await;
        self.refresh(observed, false, refresh).await
    }
    pub async fn refresh_business<F, Fut, E>(
        &self,
        observed: &Snapshot,
        refresh: F,
    ) -> Result<Snapshot, RefreshError<E>>
    where
        F: FnOnce(Snapshot) -> Fut,
        Fut: Future<Output = Result<Secret, E>>,
    {
        let _single = self.business_refresh.lock().await;
        self.refresh(observed, true, refresh).await
    }
    async fn refresh<F, Fut, E>(
        &self,
        observed: &Snapshot,
        business: bool,
        refresh: F,
    ) -> Result<Snapshot, RefreshError<E>>
    where
        F: FnOnce(Snapshot) -> Fut,
        Fut: Future<Output = Result<Secret, E>>,
    {
        let before = self.snapshot().await.ok_or(RefreshError::LoggedOut)?;
        if before.generation != observed.generation {
            return Err(RefreshError::Stale);
        }
        let target_changed = if business {
            before.business.expose() != observed.business.expose()
        } else {
            before.nim.expose() != observed.nim.expose()
        };
        if target_changed {
            return Ok(before);
        }
        let next = refresh(before.clone())
            .await
            .map_err(RefreshError::Provider)?;
        let mut s = self.state.lock().await;
        if s.current.is_none() {
            return Err(RefreshError::LoggedOut);
        }
        // Conservative rejection: any concurrent credential change may affect derivation.
        if s.generation != before.generation || s.revision != before.revision {
            return Err(RefreshError::Stale);
        }
        let pair = s.current.as_mut().unwrap();
        if business {
            pair.0 = next
        } else {
            pair.1 = next
        }
        s.revision += 1;
        let (business, nim) = s.current.as_ref().unwrap();
        Ok(Snapshot {
            generation: s.generation,
            revision: s.revision,
            business: business.clone(),
            nim: nim.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn secret(s: &str) -> Secret {
        Secret::new(s.into())
    }
    #[tokio::test]
    async fn same_generation_refresh_is_coalesced_and_preserves_business() {
        let c = CredentialCoordinator::default();
        c.login(secret("business"), secret("old")).await;
        let observed = c.snapshot().await.unwrap();
        let (a, b) = tokio::join!(
            c.refresh_nim(&observed, |_| async { Ok::<_, ()>(secret("new")) }),
            c.refresh_nim(&observed, |_| async {
                panic!("duplicate refresh");
                #[allow(unreachable_code)]
                Ok::<_, ()>(secret("wrong"))
            })
        );
        assert_eq!(a.unwrap().nim.expose(), "new");
        assert_eq!(b.unwrap().business.expose(), "business");
        assert!(!c.is_current(&observed).await);
    }
    #[tokio::test]
    async fn logout_during_refresh_cannot_restore_session() {
        let c = CredentialCoordinator::default();
        c.login(secret("b"), secret("n")).await;
        let observed = c.snapshot().await.unwrap();
        let result = c
            .refresh_nim(&observed, |_| async {
                c.logout().await;
                Ok::<_, ()>(secret("late"))
            })
            .await;
        assert!(matches!(result, Err(RefreshError::LoggedOut)));
        assert!(c.snapshot().await.is_none());
    }
    #[tokio::test]
    async fn new_login_and_cross_kind_refresh_invalidate_late_result() {
        let c = CredentialCoordinator::default();
        c.login(secret("b"), secret("n")).await;
        let old = c.snapshot().await.unwrap();
        let result = c
            .refresh_nim(&old, |_| async {
                c.login(secret("new-account"), secret("new-token")).await;
                Ok::<_, ()>(secret("late"))
            })
            .await;
        assert!(matches!(result, Err(RefreshError::Stale)));
        assert_eq!(c.snapshot().await.unwrap().nim.expose(), "new-token");
        let current = c.snapshot().await.unwrap();
        let result = c
            .refresh_nim(&current, |_| async {
                c.refresh_business(&current, |_| async { Ok::<_, ()>(secret("renewed")) })
                    .await
                    .unwrap();
                Ok::<_, ()>(secret("derived-from-old"))
            })
            .await;
        assert!(matches!(result, Err(RefreshError::Stale)));
        assert_eq!(c.snapshot().await.unwrap().business.expose(), "renewed");
    }
}
