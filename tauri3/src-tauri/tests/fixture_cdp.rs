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
            .env("DH_FIXTURE_HEADLESS", "1")
            .spawn()
            .unwrap(),
    ));
    let gateway = CdpGateway::new(CdpClient::new(devtools_base).unwrap());
    gateway.calibrate_capabilities(
        "3.0.0-fixture",
        "20fd7fecb2ec4573a7c225ecc45a14185ee3400b1097d166aa3a42984d8613ec",
    );
    let diagnostic = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let diagnostic = gateway.diagnose().await;
            if diagnostic.status == ConnectionStatus::Ready
                && diagnostic.page_title.contains("旺商聊")
            {
                break diagnostic;
            }
            assert!(tokio::time::Instant::now() < deadline, "{diagnostic:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
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
    let listener = gateway.install_message_listener().await.unwrap();
    assert_eq!(listener["ok"], true);
    let first_listener_session = listener["session"].as_str().unwrap().to_string();
    let groups = gateway.list_groups().await.unwrap();
    assert_eq!(groups.len(), 2);
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
    let batch =
        {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let batch = gateway.read_batch().await.unwrap();
                if batch.records.iter().any(|record| {
                    record.payload["idServer"].as_str() == Some("fixture-out-of-order")
                }) {
                    break batch;
                }
                assert!(tokio::time::Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
    let acknowledged_through = batch
        .records
        .iter()
        .map(|record| record.sequence)
        .max()
        .unwrap();
    gateway
        .ack(&batch.session, acknowledged_through)
        .await
        .unwrap();

    client
        .post(format!("{http_base}/fixture/faults"))
        .json(&json!({"devtoolsReady":true,"nimReady":false,"timeoutNext":false,"permissionDenied":false,"partialMembers":false}))
        .send()
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if gateway.diagnose().await.status == ConnectionStatus::NimNotReady {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let nim_error = gateway.session_identity().await.unwrap_err();
    assert_eq!(nim_error.code, "nim_not_ready");
    assert!(nim_error.retryable);

    client
        .post(format!("{http_base}/fixture/faults"))
        .json(&json!({"devtoolsReady":true,"nimReady":true,"timeoutNext":false,"permissionDenied":false,"partialMembers":true}))
        .send()
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if gateway.diagnose().await.status == ConnectionStatus::Ready {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let partial = gateway.list_members(FIXTURE_GROUP).await.unwrap();
    assert!(!partial.complete);
    assert_eq!(partial.resolved_count, 8);
    assert_eq!(partial.reported_count, 17);

    let unmute = gateway.unmute(FIXTURE_GROUP, 10006).await.unwrap();
    assert_eq!(unmute.route, "/v1/group/member-mute-cancel");
    let recall = gateway
        .recall(FIXTURE_GROUP, 10006, "fixture-contract-recall")
        .await
        .unwrap();
    assert_eq!(recall.message_id, "fixture-contract-recall");
    gateway.set_group_mute(FIXTURE_GROUP, true).await.unwrap();
    gateway.set_group_mute(FIXTURE_GROUP, false).await.unwrap();
    gateway.remove_member(FIXTURE_GROUP, 10017).await.unwrap();

    client
        .post(format!("{http_base}/fixture/reset"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    gateway.cdp().evaluate("location.reload()").await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if gateway.diagnose().await.status == ConnectionStatus::Ready {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let reinstalled = gateway.install_message_listener().await.unwrap();
    assert_eq!(reinstalled["ok"], true);
    assert_ne!(
        reinstalled["session"].as_str().unwrap(),
        first_listener_session
    );
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
    let first = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let batch = gateway.read_batch().await.unwrap();
            if batch.records.len() == 100 {
                break batch;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    assert_eq!(first.records.len(), 100);
    assert_eq!(first.records.last().unwrap().sequence, 100);
    gateway.ack(&first.session, 100).await.unwrap();
    let second = gateway.read_batch().await.unwrap();
    assert_eq!(second.records.len(), 1);
    assert_eq!(second.records[0].sequence, 101);
    gateway
        .ack(&second.session, second.records[0].sequence)
        .await
        .unwrap();

    client
        .post(format!("{http_base}/fixture/events/burst"))
        .json(&json!({"version":1,"count":1000,"startSequence":10_000}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut received = 0_usize;
    let mut previous_bridge_sequence = 0_u64;
    while received < 1000 {
        let batch = gateway.read_batch().await.unwrap();
        assert_eq!(batch.dropped, 0, "1000 条突发消息不得溢出");
        assert!(batch.records.len() <= 100, "CDP 每批上限必须为 100");
        if batch.records.is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "1000 条突发消息读取超时"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        }
        for record in &batch.records {
            assert!(record.sequence > previous_bridge_sequence);
            previous_bridge_sequence = record.sequence;
        }
        received += batch.records.len();
        let through = batch.records.last().unwrap().sequence;
        gateway.ack(&batch.session, through).await.unwrap();
    }
    assert_eq!(received, 1000);
    assert!(gateway.read_batch().await.unwrap().records.is_empty());

    client
        .post(format!("{http_base}/fixture/faults"))
        .json(&json!({"devtoolsReady":false,"nimReady":true,"timeoutNext":false,"permissionDenied":false,"partialMembers":false}))
        .send()
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if gateway.diagnose().await.status == ConnectionStatus::Unavailable {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Fixture DevTools 未断开"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    client
        .post(format!("{http_base}/fixture/faults"))
        .json(&json!({"devtoolsReady":true,"nimReady":true,"timeoutNext":false,"permissionDenied":false,"partialMembers":false}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if gateway.diagnose().await.status == ConnectionStatus::Ready {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Fixture DevTools 未恢复"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let after_reconnect = gateway.install_message_listener().await.unwrap();
    assert_eq!(after_reconnect["ok"], true);
    process.0.as_mut().unwrap().kill().unwrap();
}
