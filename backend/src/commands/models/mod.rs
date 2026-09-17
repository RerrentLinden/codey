//! Model management commands, split by concern. Every submodule keeps
//! `pub(crate)` items so the glob re-exports below present the same flat API
//! that `commands.rs` consumed when this was a single 3.5K-line file.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};

use super::{
    AppState, STARTUP_PROVIDER_MODEL_SYNC_TIMEOUT, SubagentHotReloadOutcome,
    ensure_local_route_config_writable, hot_reload_runtime_subagent_config, redacted_config,
    runtime_config_requires_restart, save_config_to_store,
};
use crate::cdp;
use crate::codex_config::codex_home;
use crate::codex_provider;
use crate::config::{
    CodeyConfig, OFFICIAL_ROUTE_SHORT_NAME, ProviderProfile, validate_provider_profiles,
};
use crate::error_log;
use crate::local_router;
use crate::model_catalog;
use crate::model_id;
use crate::provider_models;
use crate::subagent_policy;

// Record only stage names and durations, never route credentials or model payloads.
pub(crate) struct ModelOperationTimings {
    started: std::time::Instant,
    previous: std::time::Instant,
    detail: serde_json::Map<String, Value>,
}

impl ModelOperationTimings {
    pub(crate) fn new(operation: &'static str) -> Self {
        let started = std::time::Instant::now();
        Self {
            started,
            previous: started,
            detail: serde_json::Map::from_iter([("operation".into(), json!(operation))]),
        }
    }

    pub(crate) fn mark(&mut self, stage: &'static str) {
        let now = std::time::Instant::now();
        self.detail.insert(
            stage.into(),
            json!(now.duration_since(self.previous).as_millis() as u64),
        );
        self.previous = now;
    }
}

impl Drop for ModelOperationTimings {
    fn drop(&mut self) {
        self.detail.insert(
            "totalMs".into(),
            json!(self.started.elapsed().as_millis() as u64),
        );
        let _ = codey_runtime_core::diagnostic_log::append_diagnostic_log(
            "models.operation_timings",
            Value::Object(std::mem::take(&mut self.detail)),
        );
    }
}

mod catalog_refresh;
mod defaults;
mod native;
mod routes;
mod selection;
mod state;
mod sync;
#[cfg(test)]
mod tests;

pub(crate) use catalog_refresh::*;
pub use defaults::*;
pub(crate) use native::*;
pub use routes::*;
pub use selection::*;
pub(crate) use state::*;
pub use sync::*;
