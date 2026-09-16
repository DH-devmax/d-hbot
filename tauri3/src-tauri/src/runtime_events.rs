//! Business events are independent of the desktop window lifecycle.
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
#[cfg(not(feature = "headless"))]
use tauri::Emitter;

pub trait RuntimeEventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

#[derive(Default)]
pub struct NoopEventSink;
impl RuntimeEventSink for NoopEventSink {
    fn emit(&self, _: &str, _: Value) {}
}

#[derive(Clone)]
pub struct RuntimeHost(Arc<dyn RuntimeEventSink>);
impl RuntimeHost {
    pub fn new(events: Arc<dyn RuntimeEventSink>) -> Self {
        Self(events)
    }
    pub fn emit<T: Serialize>(&self, event: &str, payload: T) -> Result<(), serde_json::Error> {
        self.0.emit(event, serde_json::to_value(payload)?);
        Ok(())
    }
}

#[cfg(not(feature = "headless"))]
pub struct TauriEventSink(tauri::AppHandle);
#[cfg(not(feature = "headless"))]
impl TauriEventSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self(app)
    }
}
#[cfg(not(feature = "headless"))]
impl RuntimeEventSink for TauriEventSink {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.0.emit(event, payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, Value)>>);
    impl RuntimeEventSink for Recorder {
        fn emit(&self, event: &str, payload: Value) {
            self.0.lock().unwrap().push((event.into(), payload));
        }
    }
    #[test]
    fn cloned_hosts_deliver_typed_events_to_the_same_sink() {
        let sink = Arc::new(Recorder::default());
        let host = RuntimeHost::new(sink.clone());
        host.clone()
            .emit("connection-status", serde_json::json!({"status":"ready"}))
            .unwrap();
        assert_eq!(
            sink.0.lock().unwrap()[0],
            (
                "connection-status".into(),
                serde_json::json!({"status":"ready"})
            )
        );
    }
}
