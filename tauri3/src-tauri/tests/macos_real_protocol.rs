#![cfg(target_os = "macos")]

use dh_bot_lib::gateway::{CapabilityStatus, CdpClient, CdpGateway, GroupGateway, RuntimeGateway};
use serde_json::json;
use tokio::time::{sleep, Duration};

#[tokio::test]
#[ignore = "read-only listener diagnostics for a logged-in local WangShangLiao instance"]
async fn reports_real_listener_hook_state_without_writes() {
    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let state = gateway
        .cdp()
        .evaluate(
            r#"(()=>{const state=window.__dhBridgeMessages;const nim=window.nim;return{hasNim:Boolean(nim),version:state&&state.version,session:state&&state.session,queued:state&&state.queue&&state.queue.length||0,dropped:state&&state.dropped||0,reinstallCount:state&&state.reinstallCount||0,hooks:state&&state.hooks?state.hooks.map(hook=>({name:hook.name,attached:Boolean(nim&&nim.options&&nim.options[hook.name]===hook.wrapper),currentType:typeof(nim&&nim.options&&nim.options[hook.name]),originalType:typeof(hook.wrapper&&hook.wrapper.original)})):[]};})()"#,
        )
        .await
        .expect("listener hook state should be readable");
    eprintln!("real listener hook state: {state}");
    assert_eq!(
        state.get("hasNim").and_then(|value| value.as_bool()),
        Some(true)
    );
}

#[tokio::test]
#[ignore = "requires a logged-in local WangShangLiao instance on 127.0.0.1:9222"]
async fn probes_logged_in_macos_wangshangliao_without_writes() {
    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let capabilities = gateway
        .probe_capabilities("2.7.7-macos", "macos-local-runtime-probe")
        .await;
    let listener = gateway
        .install_message_listener()
        .await
        .expect("NIM listener should install");
    assert_eq!(
        listener.get("ok").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        listener.get("dropped").and_then(|value| value.as_u64()),
        Some(0)
    );

    let groups = gateway.list_groups().await.expect("group list should load");
    assert!(!groups.is_empty(), "logged-in account should expose groups");
    let roster = gateway
        .list_members(groups[0].group_id)
        .await
        .expect("first group roster should load");
    assert!(
        !roster.members.is_empty(),
        "first group should expose members"
    );

    if capabilities.send_text.status != CapabilityStatus::Supported {
        let sender = gateway
            .session_identity()
            .await
            .expect("session identity should load")
            .0;
        let input = json!({
            "payload": {
                "from": {"id": sender},
                "to": {"id": groups[0].group_id},
                "msgDevice": 1,
                "createdAt": {"seconds": chrono::Utc::now().timestamp(), "nanos": 0},
                "msgSession": 2,
                "msgVersion": 2,
                "accountType": 0,
                "msgFormat": 0,
                "msgRole": 0,
                "msgRingtone": 0,
                "appoint": 0,
                "content": {"data": "DH_PROTOCOL_PROBE"}
            }
        });
        let expression = format!(
            r#"(async()=>{{const input={input};const ipc=require("electron").ipcRenderer;const key="dh-macos-probe-"+Date.now();return new Promise(resolve=>{{const timer=setTimeout(()=>resolve({{timedOut:true}}),5000);ipc.once(key,(event,value)=>{{clearTimeout(timer);resolve({{code:value&&value.code,errno:value&&value.errno,responseType:typeof(value&&value.response),responseLength:typeof(value&&value.response)==="string"?value.response.length:0,message:value&&value.message}});}});ipc.send("xclient",{{type:"encode",params:JSON.stringify(input.payload),key}});}});}})()"#
        );
        let raw = gateway
            .cdp()
            .evaluate(&expression)
            .await
            .expect("encode diagnostic should return");
        eprintln!("macOS encode-only receipt: {raw}");
    }

    eprintln!(
        "macOS capability probe: send={:?} ({}) announcement={:?} ({}) member_events={:?} ({})",
        capabilities.send_text.status,
        capabilities.send_text.reason,
        capabilities.announcement.status,
        capabilities.announcement.reason,
        capabilities.member_events.status,
        capabilities.member_events.reason,
    );

    assert_eq!(capabilities.mute.status, CapabilityStatus::Supported);
    assert_eq!(capabilities.recall.status, CapabilityStatus::Supported);
    assert_eq!(capabilities.rename.status, CapabilityStatus::Supported);
    assert_eq!(
        capabilities.remove_member.status,
        CapabilityStatus::Supported
    );
    assert_eq!(capabilities.group_mute.status, CapabilityStatus::Supported);
    assert_eq!(capabilities.send_text.status, CapabilityStatus::Supported);
    assert_eq!(
        capabilities.announcement.status,
        CapabilityStatus::ManualVerification
    );
    assert_eq!(
        capabilities.member_events.status,
        CapabilityStatus::Supported
    );

    eprintln!(
        "macOS real probe passed: groups={}, first_roster={}, completeness={:?}, announcement={:?}, member_events={:?}",
        groups.len(),
        roster.members.len(),
        roster.completeness,
        capabilities.announcement.status,
        capabilities.member_events.status,
    );
}

#[tokio::test]
#[ignore = "requires DH_REAL_READ_GROUP_ID and a logged-in local WangShangLiao"]
async fn reports_configured_roster_card_names_without_writes() {
    let group_id = std::env::var("DH_REAL_READ_GROUP_ID")
        .expect("DH_REAL_READ_GROUP_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_READ_GROUP_ID must be an integer");
    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let groups = gateway.list_groups().await.expect("group list should load");
    let group = groups
        .iter()
        .find(|group| group.group_id == group_id)
        .expect("configured group should exist");
    let roster = gateway
        .list_members(group_id)
        .await
        .expect("configured roster should load");
    let empty_cards = roster
        .members
        .iter()
        .filter(|member| member.card_name.trim().is_empty())
        .count();
    let different_names = roster
        .members
        .iter()
        .filter(|member| member.card_name.trim() != member.nickname.trim())
        .count();
    eprintln!(
        "configured roster read-only report: group={}, members={}, empty_cards={}, card_nickname_differences={}, sources={:?}, complete={}",
        group.name,
        roster.members.len(),
        empty_cards,
        different_names,
        roster.sources,
        roster.complete,
    );
    assert!(roster.complete, "configured roster must be authoritative");
}

#[tokio::test]
#[ignore = "requires explicit DH_REAL_READ_GROUP_ID/DH_REAL_READ_USER_ID and a logged-in local WangShangLiao"]
async fn reports_one_member_identity_shape_without_writes() {
    let group_id = std::env::var("DH_REAL_READ_GROUP_ID")
        .expect("DH_REAL_READ_GROUP_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_READ_GROUP_ID must be an integer");
    let user_id = std::env::var("DH_REAL_READ_USER_ID")
        .expect("DH_REAL_READ_USER_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_READ_USER_ID must be an integer");
    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let input = json!({"groupId": group_id, "userId": user_id});
    let expression = format!(
        r#"(async()=>{{const input={input};const ipc=require("electron").ipcRenderer;const request=(url,payload)=>new Promise(resolve=>{{const key="dh-member-shape-"+Date.now()+"-"+Math.random();const timer=setTimeout(()=>resolve({{timedOut:true}}),10000);ipc.once(key,(event,value)=>{{clearTimeout(timer);let response=value&&value.response;try{{if(typeof response==="string")response=JSON.parse(response);}}catch{{}}resolve({{code:value&&value.code,errno:value&&value.errno,response}});}});ipc.send("xclient",{{type:"request",requestId:key,url,excuteType:0,params:JSON.stringify(payload),key}});}});const groupWire=await request("/v1/group/get-group-list",{{v:"0"}});const groupData=groupWire.response&&groupWire.response.data||groupWire.response||{{}};const groups=[...(Array.isArray(groupData.owner)?groupData.owner:[]),...(Array.isArray(groupData.member)?groupData.member:[])];const group=groups.find(item=>Number(item.groupId)===Number(input.groupId));if(!group)return{{ok:false,error:"GROUP_NOT_FOUND"}};const memberWire=await request("/v1/group/get-group-members",{{groupId:Number(input.groupId),v:"0"}});const memberData=memberWire.response&&memberWire.response.data||memberWire.response||{{}};const httpMembers=Array.isArray(memberData.groupMemberInfo)?memberData.groupMemberInfo:[];const http=httpMembers.find(item=>Number(item.userId)===Number(input.userId));const nim=await new Promise(resolve=>{{const timer=setTimeout(()=>resolve({{timedOut:true,members:[]}}),10000);window.nim.getTeamMembers({{teamId:String(group.groupCloudId||""),done:(error,value)=>{{clearTimeout(timer);const members=Array.isArray(value)?value:Array.isArray(value&&value.members)?value.members:[];resolve({{error:error&&(error.message||String(error)),members}});}}}});}});const scalar=object=>Object.fromEntries(Object.entries(object||{{}}).filter(([,value])=>value==null||["string","number","boolean"].includes(typeof value)));return{{ok:Boolean(http),http:scalar(http),nimMembers:(nim.members||[]).map(scalar)}};}})()"#
    );
    let shape = gateway
        .cdp()
        .evaluate(&expression)
        .await
        .expect("member identity shape should load");
    assert_eq!(
        shape.get("ok").and_then(|value| value.as_bool()),
        Some(true)
    );
    eprintln!("member identity shape: {shape}");
}

#[tokio::test]
#[ignore = "requires explicit DH_REAL_SEND_* variables and a logged-in local WangShangLiao"]
async fn sends_one_explicitly_configured_real_group_message() {
    let group_id = std::env::var("DH_REAL_SEND_GROUP_ID")
        .expect("DH_REAL_SEND_GROUP_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_SEND_GROUP_ID must be an integer");
    let expected_name =
        std::env::var("DH_REAL_SEND_GROUP_NAME").expect("DH_REAL_SEND_GROUP_NAME is required");
    let expected_members = std::env::var("DH_REAL_SEND_MEMBER_COUNT")
        .expect("DH_REAL_SEND_MEMBER_COUNT is required")
        .parse::<usize>()
        .expect("DH_REAL_SEND_MEMBER_COUNT must be an integer");
    let text = std::env::var("DH_REAL_SEND_TEXT").expect("DH_REAL_SEND_TEXT is required");

    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let capabilities = gateway
        .probe_capabilities("2.7.7-macos", "macos-local-runtime-send")
        .await;
    assert_eq!(capabilities.send_text.status, CapabilityStatus::Supported);

    let groups = gateway.list_groups().await.expect("group list should load");
    let target = groups
        .iter()
        .find(|group| group.group_id == group_id)
        .expect("configured group should exist");
    assert_eq!(target.name, expected_name);
    let roster = gateway
        .list_members(group_id)
        .await
        .expect("target roster should load");
    assert_eq!(roster.members.len(), expected_members);

    let receipt = gateway
        .send_text(group_id, &text)
        .await
        .expect("message should be delivered");
    assert_eq!(receipt.status, "succeeded");
    assert!(!receipt.message_id.trim().is_empty());
    eprintln!(
        "real group message delivered: members={}, message_id={}",
        roster.members.len(),
        receipt.message_id
    );
}

#[tokio::test]
#[ignore = "requires explicit DH_REAL_HISTORY_* variables and only reads recent NIM history"]
async fn finds_one_explicitly_marked_recent_group_message_without_writes() {
    let group_id = std::env::var("DH_REAL_HISTORY_GROUP_ID")
        .expect("DH_REAL_HISTORY_GROUP_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_HISTORY_GROUP_ID must be an integer");
    let expected_name = std::env::var("DH_REAL_HISTORY_GROUP_NAME")
        .expect("DH_REAL_HISTORY_GROUP_NAME is required");
    let marker =
        std::env::var("DH_REAL_HISTORY_MARKER").expect("DH_REAL_HISTORY_MARKER is required");

    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let groups = gateway.list_groups().await.expect("group list should load");
    let group = groups
        .into_iter()
        .find(|group| group.group_id == group_id)
        .expect("configured group should exist");
    assert_eq!(group.name, expected_name);

    let input = json!({"groupId": group_id, "marker": marker});
    let expression = format!(
        r#"(async()=>{{const input={input};const ipc=require("electron").ipcRenderer;const request=(url,payload)=>new Promise(resolve=>{{const key="dh-history-probe-"+Date.now()+"-"+Math.random();const timer=setTimeout(()=>resolve({{timedOut:true}}),10000);ipc.once(key,(event,value)=>{{clearTimeout(timer);let response=value&&value.response;try{{if(typeof response==="string")response=JSON.parse(response);}}catch{{}}resolve({{code:value&&value.code,errno:value&&value.errno,response}});}});ipc.send("xclient",{{type:"request",requestId:key,url,excuteType:0,params:JSON.stringify(payload),key}});}});const wire=await request("/v1/group/get-group-list",{{v:"0"}});const data=wire.response&&wire.response.data||wire.response||{{}};const groups=[...(Array.isArray(data.owner)?data.owner:[]),...(Array.isArray(data.member)?data.member:[])];const group=groups.find(item=>Number(item.groupId)===Number(input.groupId));if(!group)return{{ok:false,error:"GROUP_NOT_FOUND"}};const teamId=String(group.groupCloudId||"");if(!teamId)return{{ok:false,error:"TEAM_ID_MISSING"}};let common=window.__dhBridgeCommon||null;try{{if(!common){{const main=Array.from(document.scripts).map(item=>item.src).find(name=>/\/main-[^/]+\.js(?:\?|$)/.test(name));if(main){{const source=await(await fetch(main)).text();const match=source.match(/zh-cn-[a-z0-9]+\.js/);if(match){{common=(await import(new URL(match[0],main).href)).D;window.__dhBridgeCommon=common;}}}}}}}}catch{{}}const history=await new Promise(resolve=>{{const timer=setTimeout(()=>resolve({{timedOut:true,msgs:[]}}),10000);window.nim.getHistoryMsgs({{scene:"team",to:teamId,limit:100,done:(error,value)=>{{clearTimeout(timer);resolve({{error:error&&(error.message||String(error)),msgs:Array.isArray(value&&value.msgs)?value.msgs:Array.isArray(value)?value:[]}});}}}});}});if(history.error)return{{ok:false,error:history.error}};for(const message of history.msgs){{let decoded=null;if(common&&message.type==="custom"&&typeof message.content==="string"){{try{{decoded=await common.decodeMsg(message.content);}}catch{{}}}}const text=String(decoded&&decoded.content&&decoded.content.data||decoded&&decoded.content||message.text||"");if(text.includes(input.marker))return{{ok:true,idServer:String(message.idServer||""),sender:String(message.from||""),flow:String(message.flow||""),textLength:text.length}};}}return{{ok:false,error:"MARKER_NOT_FOUND",count:history.msgs.length}};}})()"#
    );
    let found = gateway
        .cdp()
        .evaluate(&expression)
        .await
        .expect("recent history should load");
    assert_eq!(
        found.get("ok").and_then(|value| value.as_bool()),
        Some(true)
    );
    let message_id = found
        .get("idServer")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .expect("matching message should have a server id");
    eprintln!("recent marked group message found: message_id={message_id}");
}

#[tokio::test]
#[ignore = "requires a logged-in local WangShangLiao instance on 127.0.0.1:9222"]
async fn reports_real_group_mute_wire_shape_without_writes() {
    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let expression = r#"(async()=>{const ipc=require("electron").ipcRenderer;const key="dh-mute-shape-"+Date.now();const wire=await new Promise(resolve=>{const timer=setTimeout(()=>resolve({timedOut:true}),5000);ipc.once(key,(event,value)=>{clearTimeout(timer);let response=value&&value.response;try{if(typeof response==="string")response=JSON.parse(response);}catch{}resolve({code:value&&value.code,errno:value&&value.errno,data:response&&response.data||response||{}});});ipc.send("xclient",{type:"request",requestId:key,url:"/v1/group/get-group-list",excuteType:0,params:JSON.stringify({v:"0"}),key});});const groups=[...(Array.isArray(wire.data&&wire.data.owner)?wire.data.owner:[]),...(Array.isArray(wire.data&&wire.data.member)?wire.data.member:[])];const teamStates=await Promise.all(groups.map(group=>new Promise(resolve=>{if(!window.nim||typeof window.nim.getTeam!=="function"){resolve({groupId:group.groupId,available:false});return;}const timer=setTimeout(()=>resolve({groupId:group.groupId,timedOut:true}),3000);window.nim.getTeam({teamId:String(group.groupCloudId||""),done:(error,team)=>{clearTimeout(timer);resolve({groupId:group.groupId,error:error&&String(error.message||error),keys:team&&Object.keys(team).sort(),muteFields:team&&Object.fromEntries(Object.entries(team).filter(([name])=>/mute|speak|shut/i.test(name)))});}});})));return{code:wire.code,errno:wire.errno,getTeamAvailable:!!(window.nim&&typeof window.nim.getTeam==="function"),groups:groups.map(group=>({groupId:group.groupId,groupState:group.groupState,me:group.me,keys:Object.keys(group).sort(),muteFields:Object.fromEntries(Object.entries(group).filter(([name])=>/mute|speak|shut/i.test(name)))})),teamStates};})()"#;
    let raw = gateway
        .cdp()
        .evaluate(expression)
        .await
        .expect("group list shape should load");
    eprintln!("group mute wire shape: {raw}");
}

#[tokio::test]
#[ignore = "requires a logged-in local WangShangLiao instance on 127.0.0.1:9222"]
async fn reports_real_nim_recall_methods_without_writes() {
    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let expression = r#"(()=>{const nim=window.nim;if(!nim)return{nimReady:false};const names=new Set();let current=nim;for(let depth=0;current&&depth<6;depth++,current=Object.getPrototypeOf(current)){for(const name of Object.getOwnPropertyNames(current))names.add(name);}const methods=Array.from(names).filter(name=>typeof nim[name]==="function"&&/delete|recall|withdraw|history|msg/i.test(name)).sort();const signatures={};for(const name of ["recallMsg","deleteMsg","getMsgsByIdServer","getHistoryMsgs","getLocalMsgByIdClient","findMsg"]){if(typeof nim[name]==="function")signatures[name]={length:nim[name].length,source:String(nim[name]).slice(0,1600)};}return{nimReady:true,methods,signatures};})()"#;
    let raw = gateway
        .cdp()
        .evaluate(expression)
        .await
        .expect("NIM method diagnostic should load");
    eprintln!("NIM recall-related methods: {raw}");
}

#[tokio::test]
#[ignore = "requires explicit DH_REAL_RECALL_* variables and a logged-in local WangShangLiao"]
async fn recalls_one_explicitly_configured_self_message() {
    let group_id = std::env::var("DH_REAL_RECALL_GROUP_ID")
        .expect("DH_REAL_RECALL_GROUP_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_RECALL_GROUP_ID must be an integer");
    let message_id =
        std::env::var("DH_REAL_RECALL_MESSAGE_ID").expect("DH_REAL_RECALL_MESSAGE_ID is required");

    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let capabilities = gateway
        .probe_capabilities("2.7.7-macos", "macos-local-runtime-recall")
        .await;
    assert_eq!(capabilities.recall.status, CapabilityStatus::Supported);
    let sender_user_id = gateway
        .session_identity()
        .await
        .expect("session identity should load")
        .0;
    let receipt = gateway
        .recall(group_id, sender_user_id, &message_id)
        .await
        .expect("self message should be recalled through NIM");
    assert_eq!(receipt.status, "succeeded");
    assert_eq!(receipt.route, "nim.recallMsg");
    assert_eq!(receipt.message_id, message_id);
    assert_eq!(receipt.verification.as_deref(), Some("verified"));
    eprintln!("real self message recalled with verified NIM receipt");
}

#[tokio::test]
#[ignore = "requires explicit confirmation and a marked other-member test message"]
async fn recalls_one_explicitly_configured_other_members_test_message() {
    assert_eq!(
        std::env::var("DH_REAL_RECALL_CONFIRM").as_deref(),
        Ok("RECALL_OTHER_MEMBER"),
        "DH_REAL_RECALL_CONFIRM must be RECALL_OTHER_MEMBER"
    );
    let group_id = std::env::var("DH_REAL_RECALL_GROUP_ID")
        .expect("DH_REAL_RECALL_GROUP_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_RECALL_GROUP_ID must be an integer");
    let expected_name =
        std::env::var("DH_REAL_RECALL_GROUP_NAME").expect("DH_REAL_RECALL_GROUP_NAME is required");
    let message_id =
        std::env::var("DH_REAL_RECALL_MESSAGE_ID").expect("DH_REAL_RECALL_MESSAGE_ID is required");
    let expected_sender_user_id = std::env::var("DH_REAL_RECALL_SENDER_USER_ID")
        .expect("DH_REAL_RECALL_SENDER_USER_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_RECALL_SENDER_USER_ID must be an integer");
    let marker = std::env::var("DH_REAL_RECALL_TEXT_MARKER")
        .expect("DH_REAL_RECALL_TEXT_MARKER is required");
    assert_eq!(expected_name, "大海兼职群");
    assert!(marker.chars().count() >= 4, "test marker is too short");

    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let groups = gateway.list_groups().await.expect("group list should load");
    let group = groups
        .iter()
        .find(|group| group.group_id == group_id)
        .expect("configured group should exist");
    assert_eq!(group.name, expected_name);
    let (current_user_id, _) = gateway
        .session_identity()
        .await
        .expect("session identity should load");
    assert_ne!(current_user_id, expected_sender_user_id);
    let roster = gateway
        .list_members(group_id)
        .await
        .expect("target roster should load");
    let current_member = roster
        .members
        .iter()
        .find(|member| member.user_id == current_user_id)
        .expect("current account should be in the target roster");
    assert!(
        matches!(current_member.role.as_str(), "owner" | "admin"),
        "current account must be a group owner or administrator"
    );
    let expected_sender = roster
        .members
        .iter()
        .find(|member| member.user_id == expected_sender_user_id)
        .expect("expected sender should be in the target roster");

    let input = json!({
        "groupId": group_id,
        "messageId": message_id,
        "senderNimId": expected_sender.nim_id,
        "marker": marker,
    });
    let expression = format!(
        r#"(async()=>{{const input={input};const ipc=require("electron").ipcRenderer;const request=(url,payload)=>new Promise(resolve=>{{const key="dh-real-recall-guard-"+Date.now()+"-"+Math.random();const timer=setTimeout(()=>resolve({{timedOut:true}}),10000);ipc.once(key,(event,value)=>{{clearTimeout(timer);let response=value&&value.response;try{{if(typeof response==="string")response=JSON.parse(response);}}catch{{}}resolve({{code:value&&value.code,errno:value&&value.errno,response}});}});ipc.send("xclient",{{type:"request",requestId:key,url,excuteType:0,params:JSON.stringify(payload),key}});}});const wire=await request("/v1/group/get-group-list",{{v:"0"}});const data=wire.response&&wire.response.data||wire.response||{{}};const groups=[...(Array.isArray(data.owner)?data.owner:[]),...(Array.isArray(data.member)?data.member:[])];const group=groups.find(item=>Number(item.groupId)===Number(input.groupId));if(!group)return{{ok:false,error:"GROUP_NOT_FOUND"}};const teamId=String(group.groupCloudId||"");if(!teamId)return{{ok:false,error:"TEAM_ID_MISSING"}};let common=window.__dhBridgeCommon||null;try{{if(!common){{const main=Array.from(document.scripts).map(item=>item.src).find(name=>/\/main-[^/]+\.js(?:\?|$)/.test(name));if(main){{const source=await(await fetch(main)).text();const match=source.match(/zh-cn-[a-z0-9]+\.js/);if(match){{common=(await import(new URL(match[0],main).href)).D;window.__dhBridgeCommon=common;}}}}}}}}catch{{}}const history=await new Promise(resolve=>{{const timer=setTimeout(()=>resolve({{timedOut:true,msgs:[]}}),10000);window.nim.getHistoryMsgs({{scene:"team",to:teamId,limit:100,done:(error,value)=>{{clearTimeout(timer);resolve({{error:error&&(error.message||String(error)),msgs:Array.isArray(value&&value.msgs)?value.msgs:Array.isArray(value)?value:[]}});}}}});}});const message=history.msgs.find(item=>String(item&&item.idServer||"")===String(input.messageId));if(!message)return{{ok:false,error:"MESSAGE_NOT_FOUND",teamId}};let decoded=null;if(common&&message.type==="custom"&&typeof message.content==="string"){{try{{decoded=await common.decodeMsg(message.content);}}catch{{}}}}const text=String(decoded&&decoded.content&&decoded.content.data||decoded&&decoded.content||message.text||"");return{{ok:String(message.from||"")===String(input.senderNimId)&&text.includes(input.marker),error:"",teamId,idServer:String(message.idServer||""),sender:String(message.from||""),scene:String(message.scene||""),flow:String(message.flow||""),markerMatched:text.includes(input.marker)}};}})()"#
    );
    let guard = gateway
        .cdp()
        .evaluate(&expression)
        .await
        .expect("recall safety guard should load");
    assert_eq!(
        guard.get("ok").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        guard.get("idServer").and_then(|value| value.as_str()),
        Some(message_id.as_str())
    );
    assert_eq!(
        guard.get("markerMatched").and_then(|value| value.as_bool()),
        Some(true)
    );

    let capabilities = gateway
        .probe_capabilities("2.7.8-macos", "macos-other-member-recall")
        .await;
    assert_eq!(capabilities.recall.status, CapabilityStatus::Supported);
    let receipt = gateway
        .recall(group_id, expected_sender_user_id, &message_id)
        .await
        .expect("other member test message should be recalled through NIM");
    assert_eq!(receipt.status, "succeeded");
    assert_eq!(receipt.route, "nim.recallMsg");
    assert_eq!(receipt.message_id, message_id);
    assert_eq!(receipt.verification.as_deref(), Some("verified"));
    eprintln!("real other-member marked test message recalled with verified NIM receipt");
}

#[tokio::test]
#[ignore = "requires a running DH BOT runtime and explicit real-environment injection confirmation"]
async fn injects_one_explicit_message_into_the_real_listener_queue() {
    assert_eq!(
        std::env::var("DH_REAL_INJECT_CONFIRM").as_deref(),
        Ok("INJECT_FOR_RUNNING_RUNTIME"),
        "DH_REAL_INJECT_CONFIRM must be INJECT_FOR_RUNNING_RUNTIME"
    );
    let group_id = std::env::var("DH_REAL_INJECT_GROUP_ID")
        .expect("DH_REAL_INJECT_GROUP_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_INJECT_GROUP_ID must be an integer");
    let expected_name =
        std::env::var("DH_REAL_INJECT_GROUP_NAME").expect("DH_REAL_INJECT_GROUP_NAME is required");
    let sender_user_id = std::env::var("DH_REAL_INJECT_SENDER_USER_ID")
        .expect("DH_REAL_INJECT_SENDER_USER_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_INJECT_SENDER_USER_ID must be an integer");
    let message_id =
        std::env::var("DH_REAL_INJECT_MESSAGE_ID").expect("DH_REAL_INJECT_MESSAGE_ID is required");
    let text = std::env::var("DH_REAL_INJECT_TEXT").expect("DH_REAL_INJECT_TEXT is required");
    assert_eq!(expected_name, "大海兼职群");
    assert!(!message_id.trim().is_empty());
    assert!(!text.trim().is_empty());

    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let group = gateway
        .list_groups()
        .await
        .expect("group list should load")
        .into_iter()
        .find(|group| group.group_id == group_id)
        .expect("configured group should exist");
    assert_eq!(group.name, expected_name);
    let (_, account_id) = gateway
        .session_identity()
        .await
        .expect("session identity should load");
    let sender = gateway
        .list_members(group_id)
        .await
        .expect("target roster should load")
        .members
        .into_iter()
        .find(|member| member.user_id == sender_user_id)
        .expect("injected sender should exist in target group");
    assert_ne!(sender.nim_id, account_id);
    let listener = gateway
        .install_message_listener()
        .await
        .expect("real listener should be installed");
    let session = listener
        .get("session")
        .and_then(|value| value.as_str())
        .expect("listener session should be present")
        .to_string();
    let input = json!({
        "groupId": group_id,
        "from": sender.nim_id,
        "fromNick": sender.card_name,
        "idServer": message_id,
        "text": text,
    });
    let expression = format!(
        r#"(()=>{{const input={input};const state=window.__dhBridgeMessages;if(!state)return{{ok:false,error:"LISTENER_NOT_READY"}};const sequence=state.nextSeq++;state.queue.push({{session:state.session,seq:sequence,kind:"message",source:"dh-real-test-injection",payload:{{idClient:"dh-real-inject-"+sequence,idServer:String(input.idServer),scene:"team",from:String(input.from),to:String(input.groupId),groupId:Number(input.groupId),time:Date.now(),type:"text",flow:"in",content:String(input.text),msgFormat:0,fromNick:String(input.fromNick||""),sessionId:"team-"+input.groupId}}}});return{{ok:true,session:state.session,sequence,queued:state.queue.length}};}})()"#
    );
    let inserted = gateway
        .cdp()
        .evaluate(&expression)
        .await
        .expect("message should enter the real listener queue");
    assert_eq!(
        inserted.get("ok").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        inserted.get("session").and_then(|value| value.as_str()),
        Some(session.as_str())
    );
    let sequence = inserted
        .get("sequence")
        .and_then(|value| value.as_u64())
        .expect("injected sequence should be present");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let state = gateway
            .cdp()
            .evaluate("(()=>{const state=window.__dhBridgeMessages;return{session:state&&state.session,queued:state&&state.queue.length||0,contains:Boolean(state&&state.queue.some(item=>item.source==='dh-real-test-injection'))};})()")
            .await
            .expect("listener state should remain readable");
        if state.get("contains").and_then(|value| value.as_bool()) == Some(false) {
            eprintln!("real listener injection was acknowledged: sequence={sequence}");
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let cleanup = "(()=>{const state=window.__dhBridgeMessages;if(state)state.queue=state.queue.filter(item=>item.source!=='dh-real-test-injection');return{ok:true};})()";
            let _ = gateway.cdp().evaluate(cleanup).await;
            panic!("running DH BOT did not acknowledge injected sequence {sequence}");
        }
        sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "requires a running DH BOT runtime and explicit unmapped-listener injection confirmation"]
async fn acknowledges_one_unmapped_listener_event_without_external_writes() {
    assert_eq!(
        std::env::var("DH_REAL_UNMAPPED_INJECT_CONFIRM").as_deref(),
        Ok("ACK_UNMAPPED_FOR_RUNNING_RUNTIME"),
        "DH_REAL_UNMAPPED_INJECT_CONFIRM must be ACK_UNMAPPED_FOR_RUNNING_RUNTIME"
    );
    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let listener = gateway
        .install_message_listener()
        .await
        .expect("real listener should be installed");
    let session = listener
        .get("session")
        .and_then(|value| value.as_str())
        .expect("listener session should be present")
        .to_string();
    let expression = r#"(()=>{const state=window.__dhBridgeMessages;if(!state)return{ok:false,error:"LISTENER_NOT_READY"};const sequence=state.nextSeq++;state.queue.push({session:state.session,seq:sequence,kind:"message",source:"dh-real-unmapped-injection",payload:{idClient:"dh-real-unmapped-"+sequence,idServer:"",scene:"team",from:"0",to:"unmapped-team",time:Date.now(),type:"text",flow:"in",content:"DH listener unmapped diagnostic",msgFormat:0,sessionId:"team-unmapped"}});return{ok:true,session:state.session,sequence,queued:state.queue.length};})()"#;
    let inserted = gateway
        .cdp()
        .evaluate(expression)
        .await
        .expect("unmapped event should enter the real listener queue");
    assert_eq!(
        inserted.get("ok").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        inserted.get("session").and_then(|value| value.as_str()),
        Some(session.as_str())
    );
    let sequence = inserted
        .get("sequence")
        .and_then(|value| value.as_u64())
        .expect("injected sequence should be present");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let state = gateway
            .cdp()
            .evaluate("(()=>{const state=window.__dhBridgeMessages;return{contains:Boolean(state&&state.queue.some(item=>item.source==='dh-real-unmapped-injection'))};})()")
            .await
            .expect("listener state should remain readable");
        if state.get("contains").and_then(|value| value.as_bool()) == Some(false) {
            eprintln!("real unmapped listener event was acknowledged: sequence={sequence}");
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let cleanup = "(()=>{const state=window.__dhBridgeMessages;if(state)state.queue=state.queue.filter(item=>item.source!=='dh-real-unmapped-injection');return{ok:true};})()";
            let _ = gateway.cdp().evaluate(cleanup).await;
            panic!("running DH BOT did not acknowledge unmapped sequence {sequence}");
        }
        sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "requires explicit DH_REAL_SCHEDULE_* variables and two authorized real groups"]
async fn closes_and_reopens_two_real_groups_one_minute_apart() {
    assert_eq!(
        std::env::var("DH_REAL_SCHEDULE_CONFIRM").as_deref(),
        Ok("CLOSE_AND_REOPEN"),
        "DH_REAL_SCHEDULE_CONFIRM must be CLOSE_AND_REOPEN"
    );
    let expected_names = std::env::var("DH_REAL_SCHEDULE_GROUP_NAMES")
        .expect("DH_REAL_SCHEDULE_GROUP_NAMES is required")
        .split('|')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        expected_names.len(),
        2,
        "exactly two group names are required"
    );

    let gateway = CdpGateway::new(CdpClient::new("http://127.0.0.1:9222").unwrap());
    let capabilities = gateway
        .probe_capabilities("2.7.7-macos", "macos-local-runtime-schedule")
        .await;
    assert_eq!(capabilities.group_mute.status, CapabilityStatus::Supported);
    let groups = gateway.list_groups().await.expect("group list should load");
    let targets = expected_names
        .iter()
        .map(|name| {
            groups
                .iter()
                .find(|group| group.name == *name)
                .map(|group| (group.group_id, group.name.clone()))
                .unwrap_or_else(|| panic!("authorized group not found: {name}"))
        })
        .collect::<Vec<_>>();

    for (group_id, name) in &targets {
        gateway
            .set_group_mute(*group_id, false)
            .await
            .unwrap_or_else(|error| {
                panic!("failed to ensure {name} starts open: {}", error.message)
            });
        let state = gateway
            .get_group_mute_state(*group_id)
            .await
            .unwrap_or_else(|error| {
                panic!("failed to read initial state for {name}: {}", error.message)
            });
        assert!(
            !state.muted,
            "{name} should allow speaking before the timed test"
        );
    }

    let now = chrono::Utc::now();
    let close_timestamp = (now.timestamp().div_euclid(60) + 1) * 60;
    let open_timestamp = close_timestamp + 60;
    let close_at = chrono::DateTime::from_timestamp(close_timestamp, 0).unwrap();
    let open_at = chrono::DateTime::from_timestamp(open_timestamp, 0).unwrap();
    eprintln!(
        "real schedule test: close_at={}, open_at={}, groups={:?}",
        close_at.with_timezone(&chrono::Local).format("%H:%M:%S"),
        open_at.with_timezone(&chrono::Local).format("%H:%M:%S"),
        expected_names
    );

    let result: Result<(), String> = async {
        let wait_close = (close_at - chrono::Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO);
        sleep(wait_close).await;
        for (group_id, name) in &targets {
            let receipt = gateway
                .set_group_mute(*group_id, true)
                .await
                .map_err(|error| format!("关群失败 {name}: {}", error.message))?;
            let state = gateway
                .get_group_mute_state(*group_id)
                .await
                .map_err(|error| format!("关群回读失败 {name}: {}", error.message))?;
            if receipt.verification.as_deref() != Some("verified") || !state.muted {
                return Err(format!(
                    "关群状态未确认 {name}: receipt={receipt:?}, state={state:?}"
                ));
            }
            eprintln!("closed and verified: {name}");
        }

        let wait_open = (open_at - chrono::Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO);
        sleep(wait_open).await;
        for (group_id, name) in &targets {
            let receipt = gateway
                .set_group_mute(*group_id, false)
                .await
                .map_err(|error| format!("开群失败 {name}: {}", error.message))?;
            let state = gateway
                .get_group_mute_state(*group_id)
                .await
                .map_err(|error| format!("开群回读失败 {name}: {}", error.message))?;
            if receipt.verification.as_deref() != Some("verified") || state.muted {
                return Err(format!(
                    "开群状态未确认 {name}: receipt={receipt:?}, state={state:?}"
                ));
            }
            eprintln!("opened and verified: {name}");
        }
        Ok(())
    }
    .await;

    let mut cleanup_errors = Vec::new();
    for (group_id, name) in &targets {
        if let Err(error) = gateway.set_group_mute(*group_id, false).await {
            cleanup_errors.push(format!("{name}: {}", error.message));
        } else if gateway
            .get_group_mute_state(*group_id)
            .await
            .map(|state| state.muted)
            .unwrap_or(true)
        {
            cleanup_errors.push(format!("{name}: 最终仍处于全群禁言"));
        }
    }
    assert!(
        cleanup_errors.is_empty(),
        "cleanup failed: {cleanup_errors:?}"
    );
    result.unwrap_or_else(|error| panic!("real timed group test failed: {error}"));
}
