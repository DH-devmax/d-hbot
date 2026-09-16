//! Read-only diagnostic: authenticated group/member shapes; never prints identifiers or secrets.
#[allow(dead_code)]
#[path = "../src/vault.rs"]
mod vault;
use base64::{engine::general_purpose::STANDARD, Engine};
use dh_protocol::business_client::{Client, Config};
use serde_json::Value;
use zeroize::Zeroizing;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args().nth(1).ok_or("data directory required")?;
    let vault = vault::Vault::open(std::path::Path::new(&root))?;
    let d: Value = serde_json::from_slice(
        &vault
            .read("deployment.sealed")?
            .ok_or("deployment missing")?,
    )?;
    let s: Value =
        serde_json::from_slice(&vault.read("session.sealed")?.ok_or("session missing")?)?;
    let key = |name: &str| -> Result<Zeroizing<[u8; 32]>, Box<dyn std::error::Error>> {
        Ok(Zeroizing::new(
            STANDARD
                .decode(d[name].as_str().ok_or("key absent")?)?
                .try_into()
                .map_err(|_| "key size")?,
        ))
    };
    let mut c = Client::new(Config {
        origin: d["origin"].as_str().ok_or("origin missing")?.parse()?,
        headers: serde_json::from_value(d["headers"].clone())?,
        metadata: serde_json::from_value(d["metadata"].clone())?,
        signing_seed: key("signing_seed")?,
        body_key: key("body_key")?,
        account_mac: None,
    })
    .map_err(|_| "client configuration")?;
    c.restore_session(&STANDARD.decode(s["client"].as_str().ok_or("session absent")?)?)
        .map_err(|_| "session restore")?;
    let groups = c.groups().await.map_err(|_| "group query failed")?;
    let target = std::env::args().nth(2).ok_or("group name required")?;
    for v in ["owner", "member"]
        .into_iter()
        .flat_map(|k| groups[k].as_array().into_iter().flatten())
    {
        println!(
            "selected={} cloud_type={} cloud_digits={}",
            v["groupName"].as_str() == Some(target.as_str()),
            if v["groupCloudId"].is_string() {
                "string"
            } else if v["groupCloudId"].is_number() {
                "number"
            } else {
                "other"
            },
            v["groupCloudId"].as_str().map(str::len).unwrap_or(0)
        );
        if v["groupName"].as_str() == Some(target.as_str()) {
            let id = v["groupId"]
                .as_i64()
                .or_else(|| v["groupId"].as_str()?.parse().ok())
                .ok_or("group id")?;
            let members = c
                .members_page(id, None)
                .await
                .map_err(|_| "member query failed")?;
            let rows = members["groupMemberInfo"]
                .as_array()
                .ok_or("members shape")?;
            println!(
                "test member count={} first field names={:?}",
                rows.len(),
                rows.first()
                    .and_then(Value::as_object)
                    .map(|m| m.keys().collect::<Vec<_>>())
            );
            println!("own role={}",rows.iter().find(|v|v["userId"].as_i64()==c.user_id().ok()).map(|v|v["groupRole"].to_string()).unwrap_or_default());
            println!(
                "own member found={}",
                rows.iter()
                    .any(|v| v["userId"].as_i64() == c.user_id().ok())
            );
        }
    }
    Ok(())
}
