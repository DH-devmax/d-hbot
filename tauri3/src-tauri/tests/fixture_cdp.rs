#![cfg(feature = "fixture")]

use std::process::{Child, Command};
use std::time::Duration;

use dh_bot_lib::fixture::FIXTURE_GROUP;
use dh_bot_lib::gateway::{
    CapabilityStatus, CdpClient, CdpGateway, ConnectionStatus, GroupGateway, RuntimeGateway,
};
use serde_json::json;

struct FixtureProcess(Option<Child>);

impl Drop for FixtureProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[tokio::test]
async fn cdp_fixture_exercises_real_gateway_contract() {
    let binary = env!("CARGO_BIN_EXE_dh-fixture");
    let http_base = "http://127.0.0.1:51301";
    let devtools_base = "http://127.0.0.1:9234";
    let mut process = FixtureProcess(Some(
        Command::new(binary)
            .env("DH_FIXTURE_HTTP_PORT", "51301")
            .env("DH_FIXTURE_DEVTOOLS_PORT", "9234")
            .spawn()
            .unwrap(),
    ));
    tokio::time::sleep(Duration::from_millis(250)).await;
    let gateway = CdpGateway::new(CdpClient::new(devtools_base).unwrap());
    let diagnostic = gateway.diagnose().await;
    let contract: serde_json::Value =
        serde_json::from_str(include_str!("../../contracts/group_gateway_v2.json")).unwrap();
    assert_eq!(
        diagnostic.status,
        dh_bot_lib::gateway::ConnectionStatus::Ready
    );
    assert_eq!(diagnostic.page_title, contract["metadata"]["pageTitle"]);
    assert_eq!(diagnostic.page_url, format!("{http_base}/"));
    assert_eq!(
        gateway.session_identity().await.unwrap().1,
        "fixture-nim-10001"
    );
    let groups = gateway.list_groups().await.unwrap();
    assert_eq!(groups.len(), 1);
    let roster = gateway.list_members(FIXTURE_GROUP).await.unwrap();
    assert_eq!(roster.reported_count, 16);
    let rename_receipt = gateway
        .rename(
            FIXTURE_GROUP,
            &dh_bot_lib::models::MemberRef {
                user_id: Some(10006),
                nim_id: Some("fixture-nim-10006".into()),
            },
            "DH群员0001",
        )
        .await
        .unwrap();
    assert_eq!(rename_receipt.status, "succeeded");
    assert!(!rename_receipt.request_id.is_empty());
    let mute_receipt = gateway.mute(FIXTURE_GROUP, 10006, 600).await.unwrap();
    assert_eq!(mute_receipt.business_code, Some(0));
    let send_receipt = gateway
        .send_text(FIXTURE_GROUP, "Fixture 发送测试")
        .await
        .unwrap();
    assert!(!send_receipt.request_id.is_empty());
    assert!(!send_receipt.message_id.is_empty());
    assert_eq!(gateway.capabilities().rename, CapabilityStatus::Supported);
    assert_eq!(gateway.capabilities().mute, CapabilityStatus::Supported);

    let client = reqwest::Client::new();
    client
        .post(format!("{http_base}/fixture/events/member-joined"))
        .json(&json!({"version":1,"userId":10017,"name":"新成员17"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        gateway
            .list_members(FIXTURE_GROUP)
            .await
            .unwrap()
            .reported_count,
        17
    );

    client
        .post(format!("{http_base}/fixture/events/message"))
        .json(&json!({"version":1,"sequence":120,"serverMessageId":"fixture-out-of-order","text":"@DH 乱序消息"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let batch = gateway.poll_messages().await.unwrap();
    assert_eq!(batch["messages"][0]["seq"], 120);
    gateway.acknowledge_messages(120).await.unwrap();

    client
        .post(format!("{http_base}/fixture/faults"))
        .json(&json!({"devtoolsReady":true,"nimReady":false,"timeoutNext":false,"permissionDenied":false,"partialMembers":false}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        gateway.diagnose().await.status,
        ConnectionStatus::NimNotReady
    );

    client
        .post(format!("{http_base}/fixture/faults"))
        .json(&json!({"devtoolsReady":true,"nimReady":true,"timeoutNext":false,"permissionDenied":false,"partialMembers":true}))
        .send()
        .await
        .unwrap();
    let partial = gateway.list_members(FIXTURE_GROUP).await.unwrap();
    assert!(!partial.complete);
    assert_eq!(partial.resolved_count, 8);
    assert_eq!(partial.reported_count, 17);

    client
        .post(format!("{http_base}/fixture/reset"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    for sequence in 1..=101_u64 {
        client
            .post(format!("{http_base}/fixture/events/message"))
            .json(&json!({
                "version": 1,
                "sequence": sequence,
                "serverMessageId": format!("cdp-batch-{sequence}"),
                "text": format!("batch-{sequence}")
            }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    let first = gateway.read_batch().await.unwrap();
    assert_eq!(first.records.len(), 100);
    assert_eq!(first.records.last().unwrap().sequence, 100);
    gateway.ack(&first.session, 100).await.unwrap();
    let second = gateway.read_batch().await.unwrap();
    assert_eq!(second.records.len(), 1);
    assert_eq!(second.records[0].sequence, 101);
    process.0.as_mut().unwrap().kill().unwrap();
}
