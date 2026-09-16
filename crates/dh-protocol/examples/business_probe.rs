use base64::{engine::general_purpose::STANDARD, Engine};
use dh_protocol::business_client::{Client, Config};
use dh_protocol::{
    credentials::{CredentialCoordinator, Secret},
    nim_client::Client as NimClient,
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
    origin: String,
    headers: BTreeMap<String, String>,
    metadata: [u64; 14],
    signing_seed: String,
    body_key: String,
    account_mac: String,
    nim: Option<NimInput>,
}
#[derive(Deserialize)]
struct NimInput {
    endpoint: String,
    fields: BTreeMap<String, String>,
}
fn secret<const N: usize>(s: &str) -> Result<Zeroizing<[u8; N]>, String> {
    let bytes = Zeroizing::new(STANDARD.decode(s).map_err(|_| "secret_format")?);
    Ok(Zeroizing::new(
        bytes.as_slice().try_into().map_err(|_| "secret_length")?,
    ))
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("business_probe_failed: {e}");
        std::process::exit(1)
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
    let input: Input = serde_json::from_slice(&bytes).map_err(|_| "input_format")?;
    let config = Config {
        origin: input.origin.parse().map_err(|_| "origin")?,
        headers: input.headers,
        metadata: input.metadata,
        signing_seed: secret(&input.signing_seed)?,
        body_key: secret(&input.body_key)?,
        account_mac: Some(secret(&input.account_mac)?),
    };
    let mut client = Client::new(config).map_err(|e| format!("config_{e:?}"))?;
    let groups = client.groups().await.map_err(|e| format!("groups_{e:?}"))?;
    println!(
        "rust_business_groups=passed; data_object={}; source=existing_session",
        groups.is_object()
    );
    let token = client
        .refresh_nim()
        .await
        .map_err(|e| format!("refresh_{e:?}"))?;
    println!(
        "rust_business_refresh=passed; nonempty_token={}",
        !token.is_empty()
    );
    client
        .groups()
        .await
        .map_err(|e| format!("post_refresh_groups_{e:?}"))?;
    println!("rust_post_refresh_groups=passed");
    if let Some(mut nim) = input.nim {
        nim.fields.remove("token");
        let credentials = Arc::new(CredentialCoordinator::default());
        credentials
            .login(Secret::new(String::new()), Secret::new(token.to_string()))
            .await;
        let fields: Vec<_> = nim
            .fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let mut socket =
            NimClient::connect(&nim.endpoint, &fields, credentials, Duration::from_secs(15))
                .await
                .map_err(|e| format!("nim_auth_{e:?}"))?;
        println!("rust_refreshed_nim_auth=passed");
        socket
            .heartbeat(2)
            .await
            .map_err(|e| format!("nim_heartbeat_{e:?}"))?;
        println!("rust_refreshed_nim_heartbeat=passed");
        socket
            .close()
            .await
            .map_err(|e| format!("nim_close_{e:?}"))?;
    }
    Ok(())
}
