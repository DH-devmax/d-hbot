#![cfg(feature = "fixture")]

use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use dh_bot_lib::gateway::{CdpClient, CdpGateway, ConnectionStatus, RuntimeGateway};
use dh_bot_lib::models::{AiDecision, PredictionSnapshot};
use dh_bot_lib::{
    AiProviderFactory, BackendRuntime, Clock, DeterministicSemanticClassifier, FixtureAppPaths,
    FixtureDatabase, FixtureDatabaseExecutor, FixtureLogger, FixturePredictionSource,
    FixtureSecretStore, FixtureShutdownSignal, RuntimeAiConfig, RuntimeAiProvider,
    RuntimeAiRequest, RuntimeDependencies, RuntimeEventSink,
};
use serde_json::{json, Value};
use tempfile::tempdir;

struct FixtureProcess(Option<Child>);

impl Drop for FixtureProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now_utc(&self) -> DateTime<Utc> {
        self.0
    }

    fn now_local(&self) -> DateTime<Local> {
        self.0.with_timezone(&Local)
    }
}

#[derive(Default)]
struct RecordingEvents(Mutex<Vec<(String, Value)>>);

impl RuntimeEventSink for RecordingEvents {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().unwrap().push((event.into(), payload));
    }
}

struct FixedAiProvider;

#[async_trait]
impl RuntimeAiProvider for FixedAiProvider {
    async fn decide(
        &self,
        _request: &RuntimeAiRequest,
    ) -> dh_bot_lib::error::AppResult<AiDecision> {
        Ok(AiDecision {
            reply: "fixture-ai".into(),
            actions: Vec::new(),
            tasks: Vec::new(),
            confidence: 1.0,
            reason: "fixture".into(),
        })
    }
}

struct FixedAiFactory;

impl AiProviderFactory for FixedAiFactory {
    fn create(
        &self,
        _config: RuntimeAiConfig,
    ) -> dh_bot_lib::error::AppResult<Arc<dyn RuntimeAiProvider>> {
        Ok(Arc::new(FixedAiProvider))
    }
}

#[tokio::test]
async fn real_cdp_gateway_runs_through_headless_runtime_and_sqlite_effects() {
    let binary = env!("CARGO_BIN_EXE_dh-fixture");
    let http_base = "http://127.0.0.1:51302";
    let devtools_base = "http://127.0.0.1:9235";
    let _process = FixtureProcess(Some(
        Command::new(binary)
            .env("DH_FIXTURE_HTTP_PORT", "51302")
            .env("DH_FIXTURE_DEVTOOLS_PORT", "9235")
            .env("DH_FIXTURE_HEADLESS", "1")
            .spawn()
            .unwrap(),
    ));
    let gateway = Arc::new(CdpGateway::new(CdpClient::new(devtools_base).unwrap()));
    gateway.calibrate_capabilities(
        "3.0.0-fixture",
        "20fd7fecb2ec4573a7c225ecc45a14185ee3400b1097d166aa3a42984d8613ec",
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while gateway.diagnose().await.status != ConnectionStatus::Ready {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    gateway.install_message_listener().await.unwrap();

    let client = reqwest::Client::new();
    client
        .post(format!("{http_base}/fixture/reset"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    gateway.install_message_listener().await.unwrap();
    client
        .post(format!("{http_base}/fixture/events/message"))
        .json(&json!({
            "version": 1,
            "sequence": 501,
            "serverMessageId": "runtime-headless-message",
            "text": "@DH headless runtime"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let directory = tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let paths = FixtureAppPaths {
        root: root.clone(),
        v3: root.join("fixture"),
        database: root.join("fixture/dh.db"),
        secrets: root.join("fixture/secrets.dat"),
        logs: root.join("fixture/logs"),
        legacy_backups: root.join("legacy-backups"),
        runtime_mode_file: root.join("runtime-mode"),
    };
    let database = FixtureDatabaseExecutor::start(FixtureDatabase::open(&paths).unwrap()).unwrap();
    let events = Arc::new(RecordingEvents::default());
    let fixed_now = DateTime::parse_from_rfc3339("2026-07-22T08:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let runtime = BackendRuntime::with_dependencies(
        database.clone(),
        gateway.clone(),
        FixtureSecretStore::new(&paths.secrets),
        Arc::new(FixtureShutdownSignal::default()),
        FixtureLogger::new(&paths.logs),
        RuntimeDependencies {
            clock: Arc::new(FixedClock(fixed_now)),
            events: events.clone(),
            ai_factory: Arc::new(FixedAiFactory),
            semantic_classifier: Arc::new(DeterministicSemanticClassifier::default()),
            prediction_source: Arc::new(FixturePredictionSource::new(
                Vec::<PredictionSnapshot>::new(),
            )),
        },
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if runtime.headless_ingest_once().await.unwrap() == 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "events={:?}",
            events.0.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(runtime.headless_dispatch_once().await.unwrap(), 1);

    let account_id = gateway.session_identity().await.unwrap().1;
    let messages = database
        .list_messages(account_id.clone(), None, 10)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].server_message_id, "runtime-headless-message");
    assert!(messages[0].acknowledged_at.is_some());
    assert!(database
        .action_succeeded(account_id.clone(), "headless-e2e:1".into())
        .await
        .unwrap());
    let audits = database.list_audit(account_id, 10).await.unwrap();
    assert!(audits.iter().any(|event| {
        event.event == "effect_dispatched" && event.details.contains("succeeded")
    }));
    assert!(events
        .0
        .lock()
        .unwrap()
        .iter()
        .any(|(event, payload)| event == "message-received" && payload["count"] == 1));
    let fixture_actions: Value = client
        .get(format!("{http_base}/fixture/actions"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(fixture_actions
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["kind"] == "send_text"));
    database.shutdown().await.unwrap();
}
