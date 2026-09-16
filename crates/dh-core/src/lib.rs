//! Shared business sources compiled without Tauri or a browser runtime.
#[path = "../../../tauri3/src-tauri/src/conversations.rs"]
pub mod conversations;
#[path = "../../../tauri3/src-tauri/src/ai_pipeline.rs"]
pub mod ai_pipeline;
#[path = "../../../tauri3/src-tauri/src/activities.rs"]
pub mod activities;
#[path = "../../../tauri3/src-tauri/src/ai.rs"]
pub mod ai;
#[path = "../../../tauri3/src-tauri/src/build_channel.rs"]
pub mod build_channel;
#[path = "../../../tauri3/src-tauri/src/business_apps.rs"]
pub mod business_apps;
#[path = "../../../tauri3/src-tauri/src/cardnames.rs"]
pub mod cardnames;
#[path = "../../../tauri3/src-tauri/src/contracts.rs"]
pub mod contracts;
#[path = "../../../tauri3/src-tauri/src/database.rs"]
pub mod database;
#[path = "../../../tauri3/src-tauri/src/defaults.rs"]
pub mod defaults;
#[path = "../../../tauri3/src-tauri/src/diagnostics.rs"]
pub mod diagnostics;
#[path = "../../../tauri3/src-tauri/src/error.rs"]
pub mod error;
#[path = "../../../tauri3/src-tauri/src/gateway.rs"]
pub mod gateway;
#[path = "../../../tauri3/src-tauri/src/http_body.rs"]
pub mod http_body;
#[path = "../../../tauri3/src-tauri/src/knowledge.rs"]
pub mod knowledge;
#[path = "../../../tauri3/src-tauri/src/models.rs"]
pub mod models;
#[path = "../../../tauri3/src-tauri/src/moderation.rs"]
pub mod moderation;
#[path = "../../../tauri3/src-tauri/src/paths.rs"]
pub mod paths;
#[path = "../../../tauri3/src-tauri/src/prediction.rs"]
pub mod prediction;
#[path = "../../../tauri3/src-tauri/src/queue_kernel.rs"]
pub mod queue_kernel;
#[path = "../../../tauri3/src-tauri/src/repository.rs"]
pub mod repository;
#[path = "../../../tauri3/src-tauri/src/runtime/mod.rs"]
pub mod runtime;
#[path = "../../../tauri3/src-tauri/src/runtime_events.rs"]
pub mod runtime_events;
#[path = "../../../tauri3/src-tauri/src/runtime_work.rs"]
pub mod runtime_work;
#[path = "../../../tauri3/src-tauri/src/scheduler.rs"]
pub mod scheduler;
#[path = "../../../tauri3/src-tauri/src/secrets.rs"]
pub mod secrets;
#[path = "../../../tauri3/src-tauri/src/shutdown.rs"]
pub mod shutdown;
use chrono::Utc;
use serde::Deserialize;
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageQuery {
    account_id: String,
    #[serde(default)]
    group_ids: Vec<i64>,
    keyword: Option<String>,
    kind: Option<String>,
    processing_state: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditQuery {
    account_id: String,
    group_id: Option<i64>,
    user_id: Option<i64>,
    event: Option<String>,
    level: Option<String>,
    from: Option<chrono::DateTime<Utc>>,
    to: Option<chrono::DateTime<Utc>>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[path = "../../../tauri3/src-tauri/src/service.rs"]
pub mod service;

#[cfg(feature = "headless")]
pub mod headless;
#[cfg(feature = "headless")]
pub(crate) use headless::*;
#[cfg(feature = "headless")]
mod shared_commands;

#[cfg(any(test, feature = "fixture"))]
#[path = "../../../tauri3/src-tauri/src/fixture.rs"]
pub mod fixture;
