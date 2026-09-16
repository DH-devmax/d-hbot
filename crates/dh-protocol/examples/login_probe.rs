//! Human validation input on stdin; all service traffic is issued by Rust.
use base64::{engine::general_purpose::STANDARD, Engine};
use dh_protocol::{
    business_client::{Client, Config},
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
    server_key: String,
    app_key: String,
    device_id: String,
    login: serde_json::Value,
}
fn secret<const N: usize>(value: &str) -> Result<Zeroizing<[u8; N]>, String> {
    let bytes = Zeroizing::new(STANDARD.decode(value).map_err(|_| "secret_format")?);
    Ok(Zeroizing::new(
        bytes.as_slice().try_into().map_err(|_| "secret_length")?,
    ))
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("login_probe_failed: {e}");
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
    let server_key = *secret(&input.server_key)?;
    let config = Config {
        origin: input.origin.parse().map_err(|_| "origin")?,
        headers: input.headers,
        metadata: input.metadata,
        signing_seed: secret(&input.signing_seed)?,
        body_key: secret(&input.body_key)?,
        account_mac: None,
    };
    let mut client = Client::new(config).map_err(|e| format!("config_{e:?}"))?;
    let data = client
        .login(&input.login, server_key)
        .await
        .map_err(|e| format!("login_{e:?}"))?;
    println!("rust_fresh_login=passed; imported_business_credentials=false");
    let account = data
        .get("nimId")
        .filter(|v| v.is_number() || v.is_string())
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string())
        })
        .ok_or("nim_identity")?;
    client.groups().await.map_err(|e| format!("groups_{e:?}"))?;
    println!("rust_fresh_login_groups=passed");
    let token = client
        .refresh_nim()
        .await
        .map_err(|e| format!("refresh_{e:?}"))?;
    println!("rust_fresh_login_refresh=passed");
    client
        .groups()
        .await
        .map_err(|e| format!("post_refresh_groups_{e:?}"))?;
    println!("rust_fresh_post_refresh_groups=passed");
    let endpoints =
        dh_protocol::nim_client::discover(&account, &input.app_key, Duration::from_secs(15))
            .await
            .map_err(|e| format!("nim_discovery_{e:?}"))?;
    println!("rust_nim_discovery=passed");
    let credentials = Arc::new(CredentialCoordinator::default());
    credentials
        .login(Secret::new(String::new()), Secret::new(token.to_string()))
        .await;
    for attempt in 0..2 {
        let session = format!("{}-{}", input.device_id, attempt);
        let fields = [
            ("appKey", input.app_key.as_str()),
            ("account", account.as_str()),
            ("deviceId", input.device_id.as_str()),
            ("session", session.as_str()),
            ("clientType", "16"),
            ("sdkVersion", "92114"),
            ("sdkHumanVersion", "9.21.14"),
            ("protocolVersion", "1"),
            ("appLogin", "1"),
            ("os", "Mac OS"),
            ("browser", "DH Rust"),
            ("userAgent", "Native/9.21.14"),
            ("sdkType", "0"),
            ("isReactNative", "0"),
            ("customTag", ""),
        ];
        let mut last = None;
        let mut connected = None;
        for endpoint in endpoints.iter().take(4) {
            match NimClient::connect(
                endpoint,
                &fields,
                credentials.clone(),
                Duration::from_secs(15),
            )
            .await
            {
                Ok(socket) => {
                    connected = Some(socket);
                    break;
                }
                Err(error) => last = Some(error),
            }
        }
        let mut socket = connected.ok_or_else(|| format!("nim_auth_{last:?}"))?;
        println!("rust_fresh_nim_auth=passed; reconnect={}", attempt == 1);
        socket
            .heartbeat(2)
            .await
            .map_err(|e| format!("nim_heartbeat_{e:?}"))?;
        println!(
            "rust_fresh_nim_heartbeat=passed; reconnect={}",
            attempt == 1
        );
        socket
            .close()
            .await
            .map_err(|e| format!("nim_close_{e:?}"))?;
    }
    Ok(())
}
