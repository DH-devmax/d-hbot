//! Tauri command handlers, grouped by domain.
//!
//! Commands live in these submodules; `lib.rs` keeps `AppState`, the shared
//! helpers and the `dh_handlers!` registration list.

pub mod ai;
pub mod audit;
pub mod bizapps;
pub mod cards;
#[cfg(feature = "fixture")]
pub mod developer;
pub mod groups;
pub mod knowledge;
pub mod messaging;
pub mod moderation;
pub mod rules;
pub mod summaries;
pub mod system;
pub mod tasks;
pub mod wang;
