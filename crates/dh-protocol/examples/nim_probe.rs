//! Reads a research-only credential handoff from stdin. Never persists credentials.
//! A successful run proves transport authentication, not business login.
use dh_protocol::{
    credentials::{CredentialCoordinator, Secret},
    nim_client::Client,
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::{self, Read},
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Deserialize)]
struct Input {
    endpoint: String,
    fields: BTreeMap<String, String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(stage) = run().await {
        eprintln!("nim_probe_failed: {stage}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let mut bytes = Zeroizing::new(Vec::new());
    io::stdin()
        .take(131073)
        .read_to_end(&mut bytes)
        .map_err(|_| "stdin")?;
    if bytes.len() > 131072 {
        return Err("input_limit".into());
    }
    let mut input: Input = serde_json::from_slice(&bytes).map_err(|_| "input_format")?;
    let token = input.fields.remove("token").ok_or("missing_token")?;
    let credentials = Arc::new(CredentialCoordinator::default());
    credentials
        .login(Secret::new(String::new()), Secret::new(token))
        .await;
    let fields: Vec<_> = input
        .fields
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut client = Client::connect(
        &input.endpoint,
        &fields,
        credentials,
        Duration::from_secs(15),
    )
    .await
    .map_err(|e| format!("auth_{e:?}"))?;
    println!("nim_auth=passed; credential_source=official_session; business_login=not_tested");
    client
        .heartbeat(2)
        .await
        .map_err(|e| format!("heartbeat_{e:?}"))?;
    println!("nim_heartbeat=passed");
    client.close().await.map_err(|e| format!("close_{e:?}"))?;
    println!("nim_close=passed");
    Ok(())
}
