#![cfg(target_os = "macos")]

use std::time::Duration;

use dh_bot_lib::gateway::{CdpClient, CdpGateway, GroupGateway};
use dh_bot_lib::models::MemberRef;
use serde_json::{json, Value};
use tokio::time::sleep;

const DEFAULT_PRIMARY: &str = "http://127.0.0.1:9222";
const DEFAULT_SECONDARY: &str = "http://127.0.0.1:9223";

fn gateway(url: &str) -> CdpGateway {
    CdpGateway::new(CdpClient::new(url).expect("real DevTools URL should be valid"))
}

fn required_group_id() -> i64 {
    std::env::var("DH_REAL_DUAL_GROUP_ID")
        .expect("DH_REAL_DUAL_GROUP_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_DUAL_GROUP_ID must be an integer")
}

fn required_group_name() -> String {
    std::env::var("DH_REAL_DUAL_GROUP_NAME").expect("DH_REAL_DUAL_GROUP_NAME is required")
}

async fn assert_group(gateway: &CdpGateway, group_id: i64, expected_name: &str) -> usize {
    let mut groups = None;
    let mut last_error = String::new();
    for attempt in 1..=3 {
        match gateway.list_groups().await {
            Ok(value) => {
                groups = Some(value);
                break;
            }
            Err(error) => {
                last_error = error.message;
                eprintln!("group list attempt {attempt}/3 failed: {last_error}");
                if attempt < 3 {
                    sleep(Duration::from_secs(5 * attempt)).await;
                }
            }
        }
    }
    let groups = groups.unwrap_or_else(|| panic!("configured group list failed: {last_error}"));
    let group = groups
        .iter()
        .find(|group| group.group_id == group_id)
        .unwrap_or_else(|| panic!("configured group {group_id} should exist"));
    assert_eq!(group.name, expected_name);
    let roster = gateway
        .list_members(group_id)
        .await
        .expect("configured group roster should load");
    assert!(roster.complete, "configured roster must be authoritative");
    assert!(
        !roster.members.is_empty(),
        "configured roster must not be empty"
    );
    roster.members.len()
}

async fn identity_with_retry(gateway: &CdpGateway, label: &str) -> (i64, String) {
    let mut last_error = String::new();
    for attempt in 1..=3 {
        match gateway.session_identity().await {
            Ok(identity) => return identity,
            Err(error) => {
                last_error = error.message;
                eprintln!("{label} identity attempt {attempt}/3 failed: {last_error}");
                if attempt < 3 {
                    sleep(Duration::from_secs(5 * attempt)).await;
                }
            }
        }
    }
    panic!("{label} identity failed: {last_error}")
}

#[tokio::test]
#[ignore = "requires two logged-in local WangShangLiao instances on 9222 and 9223"]
async fn verifies_two_real_instances_share_the_configured_group() {
    let group_id = required_group_id();
    let group_name = required_group_name();
    let primary_url = std::env::var("DH_REAL_PRIMARY_DEVTOOLS_URL")
        .unwrap_or_else(|_| DEFAULT_PRIMARY.to_string());
    let secondary_url = std::env::var("DH_REAL_SECONDARY_DEVTOOLS_URL")
        .unwrap_or_else(|_| DEFAULT_SECONDARY.to_string());
    let primary = gateway(&primary_url);
    let secondary = gateway(&secondary_url);

    let primary_identity = identity_with_retry(&primary, "primary").await;
    let secondary_identity = identity_with_retry(&secondary, "secondary").await;
    assert_ne!(
        primary_identity.0, secondary_identity.0,
        "the two DevTools endpoints must represent different accounts"
    );
    let primary_members = assert_group(&primary, group_id, &group_name).await;
    let secondary_members = assert_group(&secondary, group_id, &group_name).await;

    eprintln!(
        "dual-instance read-only probe passed: group={}, primary_account={}, secondary_account={}, primary_members={}, secondary_members={}",
        group_name,
        primary_identity.0,
        secondary_identity.0,
        primary_members,
        secondary_members,
    );
}

#[tokio::test]
#[ignore = "requires DH_REAL_DUAL_SEND_CONFIRM=SEND_FROM_SECONDARY"]
async fn sends_marked_messages_from_the_secondary_instance() {
    assert_eq!(
        std::env::var("DH_REAL_DUAL_SEND_CONFIRM").as_deref(),
        Ok("SEND_FROM_SECONDARY"),
        "DH_REAL_DUAL_SEND_CONFIRM must be SEND_FROM_SECONDARY"
    );
    let group_id = required_group_id();
    let group_name = required_group_name();
    let text = std::env::var("DH_REAL_DUAL_SEND_TEXT").expect("DH_REAL_DUAL_SEND_TEXT is required");
    let count = std::env::var("DH_REAL_DUAL_SEND_COUNT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 8);
    let interval_ms = std::env::var("DH_REAL_DUAL_SEND_INTERVAL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(650)
        .max(500);
    let secondary_url = std::env::var("DH_REAL_SECONDARY_DEVTOOLS_URL")
        .unwrap_or_else(|_| DEFAULT_SECONDARY.to_string());
    let secondary = gateway(&secondary_url);
    assert_group(&secondary, group_id, &group_name).await;

    for index in 0..count {
        let body = if count == 1 {
            text.clone()
        } else {
            format!("{text} [{}/{}]", index + 1, count)
        };
        let receipt = secondary
            .send_text(group_id, &body)
            .await
            .expect("secondary message should be delivered");
        assert_eq!(receipt.status, "succeeded");
        assert!(!receipt.message_id.trim().is_empty());
        eprintln!(
            "secondary message delivered: index={}, message_id={}",
            index + 1,
            receipt.message_id
        );
        if index + 1 < count {
            sleep(Duration::from_millis(interval_ms)).await;
        }
    }
}

#[tokio::test]
#[ignore = "requires DH_REAL_DUAL_HISTORY_MARKER and only reads NIM history"]
async fn reports_recent_real_messages_matching_marker() {
    let group_id = required_group_id();
    let group_name = required_group_name();
    let marker = std::env::var("DH_REAL_DUAL_HISTORY_MARKER")
        .expect("DH_REAL_DUAL_HISTORY_MARKER is required");
    let primary_url = std::env::var("DH_REAL_PRIMARY_DEVTOOLS_URL")
        .unwrap_or_else(|_| DEFAULT_PRIMARY.to_string());
    let primary = gateway(&primary_url);
    assert_group(&primary, group_id, &group_name).await;

    let input = json!({"groupId":group_id,"marker":marker});
    let expression = format!(
        r#"(async()=>{{const input={input};const ipc=require("electron").ipcRenderer;const request=(url,payload)=>new Promise(resolve=>{{const key="dh-dual-history-"+Date.now()+"-"+Math.random();const timer=setTimeout(()=>resolve({{timedOut:true}}),10000);ipc.once(key,(event,value)=>{{clearTimeout(timer);let response=value&&value.response;try{{if(typeof response==="string")response=JSON.parse(response);}}catch{{}}resolve({{code:value&&value.code,errno:value&&value.errno,response}});}});ipc.send("xclient",{{type:"request",requestId:key,url,excuteType:0,params:JSON.stringify(payload),key}});}});const wire=await request("/v1/group/get-group-list",{{v:"0"}});const data=wire.response&&wire.response.data||wire.response||{{}};const groups=[...(Array.isArray(data.owner)?data.owner:[]),...(Array.isArray(data.member)?data.member:[])];const group=groups.find(item=>Number(item.groupId)===Number(input.groupId));if(!group)return{{ok:false,error:"GROUP_NOT_FOUND"}};const teamId=String(group.groupCloudId||"");if(!teamId)return{{ok:false,error:"TEAM_ID_MISSING"}};let common=window.__dhBridgeCommon||null;try{{if(!common){{const main=Array.from(document.scripts).map(item=>item.src).find(name=>/\/main-[^/]+\.js(?:\?|$)/.test(name));if(main){{const source=await(await fetch(main)).text();const match=source.match(/zh-cn-[a-z0-9]+\.js/);if(match){{common=(await import(new URL(match[0],main).href)).D;window.__dhBridgeCommon=common;}}}}}}}}catch{{}}const history=await new Promise(resolve=>{{const timer=setTimeout(()=>resolve({{timedOut:true,msgs:[]}}),10000);window.nim.getHistoryMsgs({{scene:"team",to:teamId,limit:100,done:(error,value)=>{{clearTimeout(timer);resolve({{error:error&&(error.message||String(error)),msgs:Array.isArray(value&&value.msgs)?value.msgs:Array.isArray(value)?value:[]}});}}}});}});if(history.error)return{{ok:false,error:history.error}};const matches=[];for(const message of history.msgs){{let decoded=null;if(common&&message.type==="custom"&&typeof message.content==="string"){{try{{decoded=await common.decodeMsg(message.content);}}catch{{}}}}const text=String(decoded&&decoded.content&&decoded.content.data||decoded&&decoded.content||message.text||"");if(text.includes(input.marker))matches.push({{from:String(message.from||""),idServer:String(message.idServer||""),time:Number(message.time||0),flow:String(message.flow||""),type:String(message.type||""),msgFormat:Number(decoded&&decoded.msgFormat||message.msgFormat||0),text}});}}return{{ok:true,matches}};}})()"#
    );
    let result = primary
        .cdp()
        .evaluate(&expression)
        .await
        .expect("NIM history lookup should run");
    assert_eq!(result.get("ok").and_then(Value::as_bool), Some(true));
    eprintln!("dual-instance history matches: {result}");
}

#[tokio::test]
#[ignore = "requires DH_REAL_DUAL_HISTORY_IDS and only reads NIM history"]
async fn reports_recent_real_messages_matching_server_ids() {
    let group_id = required_group_id();
    let group_name = required_group_name();
    let raw_ids =
        std::env::var("DH_REAL_DUAL_HISTORY_IDS").expect("DH_REAL_DUAL_HISTORY_IDS is required");
    let ids = raw_ids
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    assert!(!ids.is_empty());
    let primary_url = std::env::var("DH_REAL_PRIMARY_DEVTOOLS_URL")
        .unwrap_or_else(|_| DEFAULT_PRIMARY.to_string());
    let primary = gateway(&primary_url);
    assert_group(&primary, group_id, &group_name).await;

    let input = json!({"groupId":group_id,"ids":ids});
    let expression = format!(
        r#"(async()=>{{const input={input};const ipc=require("electron").ipcRenderer;const request=(url,payload)=>new Promise(resolve=>{{const key="dh-dual-history-ids-"+Date.now()+"-"+Math.random();const timer=setTimeout(()=>resolve({{timedOut:true}}),10000);ipc.once(key,(event,value)=>{{clearTimeout(timer);let response=value&&value.response;try{{if(typeof response==="string")response=JSON.parse(response);}}catch{{}}resolve({{code:value&&value.code,errno:value&&value.errno,response}});}});ipc.send("xclient",{{type:"request",requestId:key,url,excuteType:0,params:JSON.stringify(payload),key}});}});const wire=await request("/v1/group/get-group-list",{{v:"0"}});const data=wire.response&&wire.response.data||wire.response||{{}};const groups=[...(Array.isArray(data.owner)?data.owner:[]),...(Array.isArray(data.member)?data.member:[])];const group=groups.find(item=>Number(item.groupId)===Number(input.groupId));if(!group)return{{ok:false,error:"GROUP_NOT_FOUND"}};const teamId=String(group.groupCloudId||"");if(!teamId)return{{ok:false,error:"TEAM_ID_MISSING"}};const history=await new Promise(resolve=>{{const timer=setTimeout(()=>resolve({{timedOut:true,msgs:[]}}),10000);window.nim.getHistoryMsgs({{scene:"team",to:teamId,limit:100,done:(error,value)=>{{clearTimeout(timer);resolve({{error:error&&(error.message||String(error)),msgs:Array.isArray(value&&value.msgs)?value.msgs:Array.isArray(value)?value:[]}});}}}});}});if(history.error)return{{ok:false,error:history.error}};const wanted=new Set(input.ids.map(String));const matches=history.msgs.filter(message=>wanted.has(String(message&&message.idServer||""))).map(message=>({{idServer:String(message.idServer||""),idClient:String(message.idClient||""),from:String(message.from||""),flow:String(message.flow||""),type:String(message.type||""),time:Number(message.time||0)}}));return{{ok:true,matches,requested:input.ids}};}})()"#
    );
    let result = primary
        .cdp()
        .evaluate(&expression)
        .await
        .expect("real history id readback should execute");
    assert_eq!(result.get("ok").and_then(Value::as_bool), Some(true));
    eprintln!("dual-instance history id matches: {result}");
}

#[tokio::test]
#[ignore = "requires DH_REAL_DUAL_MENTION_CONFIRM=SEND_MENTION_FROM_SECONDARY"]
async fn sends_real_mention_from_the_secondary_instance() {
    assert_eq!(
        std::env::var("DH_REAL_DUAL_MENTION_CONFIRM").as_deref(),
        Ok("SEND_MENTION_FROM_SECONDARY"),
        "DH_REAL_DUAL_MENTION_CONFIRM must be SEND_MENTION_FROM_SECONDARY"
    );
    let group_id = required_group_id();
    let group_name = required_group_name();
    let target_account = std::env::var("DH_REAL_DUAL_MENTION_ACCOUNT")
        .expect("DH_REAL_DUAL_MENTION_ACCOUNT is required");
    let target_name =
        std::env::var("DH_REAL_DUAL_MENTION_NAME").expect("DH_REAL_DUAL_MENTION_NAME is required");
    let body =
        std::env::var("DH_REAL_DUAL_MENTION_TEXT").expect("DH_REAL_DUAL_MENTION_TEXT is required");
    let secondary_url = std::env::var("DH_REAL_SECONDARY_DEVTOOLS_URL")
        .unwrap_or_else(|_| DEFAULT_SECONDARY.to_string());
    let secondary = gateway(&secondary_url);
    assert_group(&secondary, group_id, &group_name).await;
    let sender = identity_with_retry(&secondary, "secondary").await.0;
    let text = format!("@{target_name} {body}");
    let end = format!("@{target_name} ").encode_utf16().count();
    let input = json!({
        "groupId": group_id,
        "sender": sender,
        "targetAccount": target_account,
        "targetName": target_name,
        "text": text,
        "mentionEnd": end,
        "createdAt": chrono::Utc::now().timestamp(),
    });
    let expression = format!(
        r#"(async()=>{{const input={input};const ipc=require("electron").ipcRenderer;const request=(url,payload)=>new Promise(resolve=>{{const key="dh-real-mention-request-"+Date.now()+"-"+Math.random();const timer=setTimeout(()=>resolve({{timedOut:true}}),10000);ipc.once(key,(event,value)=>{{clearTimeout(timer);resolve(value||{{}});}});ipc.send("xclient",{{type:"request",requestId:key,url,excuteType:0,params:JSON.stringify(payload),key}});}});const groupWire=await request("/v1/group/get-group-list",{{v:"0"}});let groupResponse=groupWire.response;try{{if(typeof groupResponse==="string")groupResponse=JSON.parse(groupResponse);}}catch{{}}const groupData=groupResponse&&groupResponse.data||groupResponse||{{}};const groups=[...(Array.isArray(groupData.owner)?groupData.owner:[]),...(Array.isArray(groupData.member)?groupData.member:[])];const group=groups.find(item=>Number(item.groupId)===Number(input.groupId));if(!group)return{{ok:false,error:"GROUP_NOT_FOUND"}};const teamId=String(group.groupCloudId||"");if(!teamId)return{{ok:false,error:"TEAM_ID_MISSING"}};const payload={{from:{{id:input.sender}},to:{{id:input.groupId}},msgDevice:1,createdAt:{{seconds:input.createdAt,nanos:0}},msgSession:2,msgVersion:2,accountType:0,msgFormat:0,msgRole:0,msgRingtone:0,appoint:1,aite:{{AiTeInfo:[{{end:input.mentionEnd,nick:input.targetName,uid:Number(input.targetAccount)}}],content:{{data:input.text,maskWords:[]}}}}}};const encoded=await new Promise(resolve=>{{const key="dh-real-mention-encode-"+Date.now()+"-"+Math.random();const timer=setTimeout(()=>resolve({{timedOut:true}}),10000);ipc.once(key,(event,value)=>{{clearTimeout(timer);resolve(value||{{}});}});ipc.send("xclient",{{type:"encode",params:JSON.stringify(payload),key}});}});if((Number(encoded.code)!==0&&Number(encoded.code)!==200)||Number(encoded.errno)!==0||typeof encoded.response!=="string")return{{ok:false,error:"ENCODE_FAILED",encoded}};return new Promise(resolve=>{{const timer=setTimeout(()=>resolve({{ok:false,error:"NIM_SEND_TIMEOUT"}}),15000);window.nim.sendCustomMsg({{scene:"team",to:teamId,content:encoded.response,isLocal:false,done:(error,message)=>{{clearTimeout(timer);resolve({{ok:!error,error:error&&(error.message||String(error)),idServer:String(message&&message.idServer||""),idClient:String(message&&message.idClient||"")}});}}}});}});}})()"#
    );
    let result = secondary
        .cdp()
        .evaluate(&expression)
        .await
        .expect("real mention should execute through WangShangLiao");
    assert_eq!(result.get("ok").and_then(Value::as_bool), Some(true));
    assert!(result
        .get("idServer")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty()));
    eprintln!("secondary real mention delivered: {result}");
}

#[tokio::test]
#[ignore = "requires DH_REAL_DUAL_IMAGE_CONFIRM=SEND_IMAGE_FROM_SECONDARY and an enabled image machine rule"]
async fn sends_tiny_image_from_the_secondary_instance() {
    assert_eq!(
        std::env::var("DH_REAL_DUAL_IMAGE_CONFIRM").as_deref(),
        Ok("SEND_IMAGE_FROM_SECONDARY"),
        "DH_REAL_DUAL_IMAGE_CONFIRM must be SEND_IMAGE_FROM_SECONDARY"
    );
    let group_id = required_group_id();
    let group_name = required_group_name();
    let secondary_url = std::env::var("DH_REAL_SECONDARY_DEVTOOLS_URL")
        .unwrap_or_else(|_| DEFAULT_SECONDARY.to_string());
    let secondary = gateway(&secondary_url);
    assert_group(&secondary, group_id, &group_name).await;
    let input = json!({"groupId": group_id});
    let expression = format!(
        r#"(async()=>{{const input={input};const ipc=require("electron").ipcRenderer;const request=(url,payload)=>new Promise(resolve=>{{const key="dh-real-image-request-"+Date.now()+"-"+Math.random();const timer=setTimeout(()=>resolve({{timedOut:true}}),10000);ipc.once(key,(event,value)=>{{clearTimeout(timer);resolve(value||{{}});}});ipc.send("xclient",{{type:"request",requestId:key,url,excuteType:0,params:JSON.stringify(payload),key}});}});const wire=await request("/v1/group/get-group-list",{{v:"0"}});let response=wire.response;try{{if(typeof response==="string")response=JSON.parse(response);}}catch{{}}const data=response&&response.data||response||{{}};const groups=[...(Array.isArray(data.owner)?data.owner:[]),...(Array.isArray(data.member)?data.member:[])];const group=groups.find(item=>Number(item.groupId)===Number(input.groupId));if(!group)return{{ok:false,error:"GROUP_NOT_FOUND"}};const teamId=String(group.groupCloudId||"");if(!teamId)return{{ok:false,error:"TEAM_ID_MISSING"}};if(!window.nim||typeof window.nim.sendFile!=="function")return{{ok:false,error:"NIM_SEND_FILE_UNAVAILABLE"}};const png=Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="),char=>char.charCodeAt(0));const blob=new Blob([png],{{type:"image/png"}});return new Promise(resolve=>{{const timer=setTimeout(()=>resolve({{ok:false,error:"NIM_SEND_FILE_TIMEOUT"}}),20000);window.nim.sendFile({{scene:"team",to:teamId,type:"image",blob,done:(error,message)=>{{clearTimeout(timer);resolve({{ok:!error,error:error&&(error.message||String(error)),idServer:String(message&&message.idServer||""),idClient:String(message&&message.idClient||""),type:String(message&&message.type||"image")}});}}}});}});}})()"#
    );
    let result = secondary
        .cdp()
        .evaluate(&expression)
        .await
        .expect("real image send should execute through WangShangLiao");
    assert_eq!(result.get("ok").and_then(Value::as_bool), Some(true));
    assert!(result
        .get("idServer")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty()));
    eprintln!("secondary image delivered: {result}");
}

async fn wait_for_own_card(
    gateway: &CdpGateway,
    group_id: i64,
    user_id: i64,
    expected: &str,
    attempts: usize,
) -> bool {
    for _ in 0..attempts {
        if gateway
            .list_members(group_id)
            .await
            .ok()
            .and_then(|roster| {
                roster
                    .members
                    .into_iter()
                    .find(|member| member.user_id == user_id)
            })
            .is_some_and(|member| member.card_name == expected)
        {
            return true;
        }
        sleep(Duration::from_secs(1)).await;
    }
    false
}

#[tokio::test]
#[ignore = "requires DH_REAL_DUAL_CARD_CONFIRM=CHANGE_AND_RESTORE_OWN_CARD and a running DH BOT on 9222"]
async fn secondary_card_change_is_restored_by_primary_dh() {
    assert_eq!(
        std::env::var("DH_REAL_DUAL_CARD_CONFIRM").as_deref(),
        Ok("CHANGE_AND_RESTORE_OWN_CARD"),
        "DH_REAL_DUAL_CARD_CONFIRM must be CHANGE_AND_RESTORE_OWN_CARD"
    );
    let group_id = required_group_id();
    let group_name = required_group_name();
    let original = std::env::var("DH_REAL_DUAL_CARD_ORIGINAL")
        .expect("DH_REAL_DUAL_CARD_ORIGINAL is required");
    let marker = std::env::var("DH_REAL_DUAL_CARD_MARKER").unwrap_or_else(|_| "改错0731".into());
    let repeat = std::env::var("DH_REAL_DUAL_CARD_REPEAT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 5);
    let restore_wait_seconds = std::env::var("DH_REAL_DUAL_CARD_RESTORE_WAIT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(75)
        .clamp(15, 120);
    let secondary_url = std::env::var("DH_REAL_SECONDARY_DEVTOOLS_URL")
        .unwrap_or_else(|_| DEFAULT_SECONDARY.to_string());
    let primary_url = std::env::var("DH_REAL_PRIMARY_DEVTOOLS_URL")
        .unwrap_or_else(|_| DEFAULT_PRIMARY.to_string());
    let primary = gateway(&primary_url);
    let secondary = gateway(&secondary_url);
    assert_group(&primary, group_id, &group_name).await;
    assert_group(&secondary, group_id, &group_name).await;
    let user_id = identity_with_retry(&secondary, "secondary").await.0;
    let target = primary
        .list_members(group_id)
        .await
        .expect("primary roster should load")
        .members
        .into_iter()
        .find(|member| member.user_id == user_id)
        .expect("secondary member should exist in primary roster");
    let member_ref = MemberRef {
        user_id: Some(user_id),
        nim_id: (!target.nim_id.is_empty()).then(|| target.nim_id.clone()),
    };

    for index in 0..repeat {
        let temporary = format!("{marker}{}", index + 1);
        let changed = primary
            .rename(group_id, &member_ref, &temporary)
            .await
            .expect("primary administrator should change the secondary card");
        assert_eq!(changed.status, "succeeded");
        assert!(
            wait_for_own_card(&secondary, group_id, user_id, &temporary, 8).await,
            "secondary card should first change to {temporary}"
        );
        if !wait_for_own_card(
            &secondary,
            group_id,
            user_id,
            &original,
            restore_wait_seconds,
        )
        .await
        {
            let cleanup = primary.rename(group_id, &member_ref, &original).await;
            panic!("DH BOT did not restore card {temporary} to {original}; cleanup={cleanup:?}");
        }
        eprintln!(
            "secondary card restored by DH BOT: attempt={}/{}, temporary={}, restored={}",
            index + 1,
            repeat,
            temporary,
            original
        );
        sleep(Duration::from_millis(650)).await;
    }
}
