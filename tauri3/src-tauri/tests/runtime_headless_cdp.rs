#![cfg(feature = "fixture")]

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

struct FixtureProcess {
    child: Option<Child>,
    http_port: u16,
}

fn request_fixture_shutdown(port: u16) {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(300)) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
    let request = format!(
        "POST /fixture/shutdown HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_ok() {
        let _ = stream.shutdown(Shutdown::Write);
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let _ = stream.read(&mut [0_u8; 256]);
    }
}

fn terminate_fixture_browser_children(fixture_pid: u32) {
    let profile_prefix = format!("dh-fixture-browser-{fixture_pid}-");
    #[cfg(windows)]
    {
        let script = format!(
            "$prefix = [regex]::Escape('{profile_prefix}'); Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -match $prefix }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}"
        );
        let _ = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("pkill")
            .args(["-TERM", "-f", &profile_prefix])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

impl Drop for FixtureProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let fixture_pid = child.id();
            request_fixture_shutdown(self.http_port);
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut exited = false;
            while Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() {
                    exited = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            terminate_fixture_browser_children(fixture_pid);
            if exited {
                return;
            }
            #[cfg(windows)]
            {
                let process_id = child.id().to_string();
                let status = Command::new("taskkill")
                    .args(["/PID", &process_id, "/T", "/F"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                if !status.is_ok_and(|status| status.success()) {
                    let _ = child.kill();
                }
            }
            #[cfg(not(windows))]
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
        _configs: Vec<RuntimeAiConfig>,
    ) -> dh_bot_lib::error::AppResult<Arc<dyn RuntimeAiProvider>> {
        Ok(Arc::new(FixedAiProvider))
    }
}

#[tokio::test]
async fn real_cdp_gateway_runs_through_headless_runtime_and_sqlite_effects() {
    let binary = env!("CARGO_BIN_EXE_dh-fixture");
    let http_base = "http://127.0.0.1:51302";
    let devtools_base = "http://127.0.0.1:9235";
    let _process = FixtureProcess {
        child: Some(
            Command::new(binary)
                .env("DH_FIXTURE_HTTP_PORT", "51302")
                .env("DH_FIXTURE_DEVTOOLS_PORT", "9235")
                .env("DH_FIXTURE_HEADLESS", "1")
                .spawn()
                .unwrap(),
        ),
        http_port: 51302,
    };
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
