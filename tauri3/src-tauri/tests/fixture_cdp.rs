#![cfg(feature = "fixture")]

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use dh_bot_lib::fixture::FIXTURE_GROUP;
use dh_bot_lib::gateway::{
    CapabilityStatus, CdpClient, CdpGateway, ConnectionStatus, GatewayRecord, GatewayRecordKind,
    GroupGateway, RuntimeGateway,
};
use serde_json::json;

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

/// 等待批次中出现满足 `predicate` 的记录。
///
/// `read_batch` 只做 `state.queue.slice(0,100)`——它不消费队列，未 ACK 的记录会
/// 一直留在队列里被重复读到。所以"批次非空"并不代表调用方等的那条记录已经到达：
/// 之前遗留的记录足以让等待立刻返回。调用方必须说明自己等的是什么。
async fn wait_for_records(
    gateway: &CdpGateway,
    timeout: Duration,
    predicate: impl Fn(&GatewayRecord) -> bool,
) -> dh_bot_lib::gateway::GatewayBatch {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let batch = gateway.read_batch().await.unwrap();
        if batch.records.iter().any(&predicate) {
            return batch;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "等待 Fixture 浏览器将 NIM 回调交给已安装监听器超时"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn cdp_fixture_exercises_real_gateway_contract() {
    let binary = env!("CARGO_BIN_EXE_dh-fixture");
    let http_base = "http://127.0.0.1:51301";
    let devtools_base = "http://127.0.0.1:9234";
    let _process = FixtureProcess {
        child: Some(
            Command::new(binary)
                .env("DH_FIXTURE_HTTP_PORT", "51301")
                .env("DH_FIXTURE_DEVTOOLS_PORT", "9234")
                .env("DH_FIXTURE_HEADLESS", "1")
                .spawn()
                .unwrap(),
        ),
        http_port: 51301,
    };
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
    let probed = gateway.probe_capabilities("2.7.8", &"f".repeat(64)).await;
    assert_eq!(probed.mute, CapabilityStatus::Supported);
    assert_eq!(probed.recall, CapabilityStatus::Supported);
    assert_eq!(probed.rename, CapabilityStatus::Supported);
    assert_eq!(probed.remove_member, CapabilityStatus::Supported);
    assert_eq!(probed.group_mute, CapabilityStatus::Supported);
    assert_eq!(probed.announcement, CapabilityStatus::ManualVerification);
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

    let cold_gateway = CdpGateway::new(CdpClient::new(devtools_base).unwrap());
    cold_gateway.calibrate_capabilities(
        "3.0.0-fixture",
        "20fd7fecb2ec4573a7c225ecc45a14185ee3400b1097d166aa3a42984d8613ec",
    );
    cold_gateway.session_identity().await.unwrap();
    cold_gateway.invalidate_group_cache().await;
    let cold_send = cold_gateway
        .send_text(FIXTURE_GROUP, "冷启动缓存缺失直写测试")
        .await
        .unwrap();
    assert_eq!(cold_send.status, "succeeded");

    let client = reqwest::Client::new();
    for (member_count, expected_pages) in [(50_usize, 1_usize), (51, 2), (1_000, 20)] {
        client
            .post(format!("{http_base}/fixture/members/resize"))
            .json(&json!({"version":1,"groupId":FIXTURE_GROUP,"count":member_count}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        gateway.invalidate_member_cache(FIXTURE_GROUP).await;
        gateway.invalidate_group_cache().await;
        let paged = gateway.list_members(FIXTURE_GROUP).await.unwrap();
        assert_eq!(paged.members.len(), member_count);
        assert_eq!(paged.http_pages, expected_pages);
        assert_eq!(paged.nim_pages, expected_pages);
        assert!(paged.complete);
    }
    client
        .post(format!("{http_base}/fixture/members/resize"))
        .json(&json!({"version":1,"groupId":FIXTURE_GROUP,"count":16}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    gateway.invalidate_member_cache(FIXTURE_GROUP).await;
    gateway.invalidate_group_cache().await;

    for (code, message) in [
        (503_i64, "NIM service unavailable"),
        (409_i64, "NIM business rejected"),
    ] {
        client
            .post(format!("{http_base}/fixture/faults"))
            .json(&json!({
                "devtoolsReady":true,
                "nimReady":true,
                "nimMembersErrorCode":code,
                "nimMembersErrorMessage":message
            }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        gateway.invalidate_member_cache(FIXTURE_GROUP).await;
        let partial = gateway.list_members(FIXTURE_GROUP).await.unwrap();
        assert!(!partial.complete);
        assert_eq!(partial.status, "partial");
        assert_eq!(partial.members.len(), 16);
        assert_eq!(partial.source_errors.len(), 1);
        assert!(partial.source_errors[0].reason.contains(&code.to_string()));
        assert_ne!(partial.completeness_reason, "NIM 成员分页超过 100 页");
    }
    client
        .post(format!("{http_base}/fixture/faults"))
        .json(&json!({"devtoolsReady":true,"nimReady":true}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    gateway.invalidate_member_cache(FIXTURE_GROUP).await;

    client
        .post(format!("{http_base}/fixture/faults"))
        .json(&json!({
            "devtoolsReady":true,
            "nimReady":true,
            "nimSendTimeoutAfterEffect":true
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let announcement = gateway
        .set_group_announcement(FIXTURE_GROUP, "公告广播幂等测试")
        .await
        .unwrap();
    assert_eq!(announcement.status, "unknown");
    assert_eq!(announcement.verification.as_deref(), Some("unknown"));
    let actions_after_timeout = client
        .get(format!("{http_base}/fixture/actions"))
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();
    let announcement_writes = actions_after_timeout
        .iter()
        .filter(|action| action["kind"] == "group_announcement")
        .count();
    let announcement_broadcasts = actions_after_timeout
        .iter()
        .filter(|action| action["kind"] == "send_text")
        .count();
    let repeated = gateway
        .set_group_announcement(FIXTURE_GROUP, "公告广播幂等测试")
        .await
        .unwrap();
    assert_eq!(repeated.verification.as_deref(), Some("unknown"));
    let actions_after_retry = client
        .get(format!("{http_base}/fixture/actions"))
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();
    assert_eq!(
        actions_after_retry
            .iter()
            .filter(|action| action["kind"] == "group_announcement")
            .count(),
        announcement_writes + 1
    );
    assert_eq!(
        actions_after_retry
            .iter()
            .filter(|action| action["kind"] == "send_text")
            .count(),
        announcement_broadcasts + 1
    );
    client
        .post(format!("{http_base}/fixture/faults"))
        .json(&json!({"devtoolsReady":true,"nimReady":true}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

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
    client
        .post(format!("{http_base}/fixture/events/message"))
        .json(&json!({
            "version":1,
            "groupId":FIXTURE_GROUP,
            "userId":10006,
            "sequence":91,
            "serverMessageId":"fixture-other-member-recall",
            "text":"请提供验证码"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    wait_for_records(&gateway, Duration::from_secs(5), |record| {
        record.payload["idServer"].as_str() == Some("fixture-other-member-recall")
    })
    .await;
    let recall_receipt = gateway
        .recall(FIXTURE_GROUP, 10006, "fixture-other-member-recall")
        .await
        .unwrap();
    assert_eq!(recall_receipt.route, "nim.recallMsg");
    assert_eq!(recall_receipt.message_id, "fixture-other-member-recall");
    assert_eq!(recall_receipt.verification.as_deref(), Some("verified"));
    let recall_actions = client
        .get(format!("{http_base}/fixture/actions"))
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();
    assert!(recall_actions.iter().any(|action| {
        action["kind"] == "recall"
            && action["userId"] == 10006
            && action["text"] == "fixture-other-member-recall"
    }));
    assert_eq!(gateway.capabilities().rename, CapabilityStatus::Supported);
    assert_eq!(gateway.capabilities().mute, CapabilityStatus::Supported);

    client
        .post(format!("{http_base}/fixture/events/member-joined"))
        .json(&json!({"version":1,"userId":10017,"name":"新成员17"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let member_batch = wait_for_records(&gateway, Duration::from_secs(5), |record| {
        record.kind == GatewayRecordKind::TeamMemberJoined
    })
    .await;
    assert_eq!(
        gateway
            .member_events(member_batch.records.clone())
            .await
            .unwrap()
            .len(),
        1
    );
    gateway
        .ack(
            &member_batch.session,
            member_batch.records.last().unwrap().sequence,
        )
        .await
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
    assert!(partial.complete);
    assert_eq!(partial.authority, "authoritative");
    assert_eq!(partial.resolved_count, 8);
    assert_eq!(partial.reported_count, 8);

    let unmute = gateway.unmute(FIXTURE_GROUP, 10006).await.unwrap();
    assert_eq!(unmute.verification.as_deref(), Some("verified"));
    let missing_recall = gateway
        .recall(FIXTURE_GROUP, 10006, "fixture-contract-recall")
        .await
        .unwrap_err();
    assert_eq!(missing_recall.code, "nim_recall");
    assert!(!missing_recall.retryable);
    let group_mute = gateway.set_group_mute(FIXTURE_GROUP, true).await.unwrap();
    assert_eq!(group_mute.verification.as_deref(), Some("verified"));
    let remove = gateway
        .remove_member(FIXTURE_GROUP, 10017)
        .await
        .unwrap_err();
    assert_eq!(remove.code, "member_not_found");

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
}
