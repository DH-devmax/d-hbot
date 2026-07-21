use tokio::sync::watch;

/// A latched shutdown signal. Subscribers created after cancellation observe
/// the cancelled state immediately, unlike an edge-triggered notification.
#[derive(Debug)]
pub struct ShutdownSignal {
    sender: watch::Sender<bool>,
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        let (sender, _) = watch::channel(false);
        Self { sender }
    }
}

impl ShutdownSignal {
    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        if *receiver.borrow_and_update() {
            return;
        }
        while receiver.changed().await.is_ok() {
            if *receiver.borrow_and_update() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_is_visible_to_late_subscribers() {
        let signal = ShutdownSignal::default();
        signal.cancel();
        tokio::time::timeout(std::time::Duration::from_millis(50), signal.cancelled())
            .await
            .expect("latched cancellation should complete immediately");
        assert!(signal.is_cancelled());
    }
}
