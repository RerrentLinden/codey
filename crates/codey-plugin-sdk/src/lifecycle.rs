//! ABI v1 上的可选请求生命周期协议。宿主控制等待、重试和最终响应提交。
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const CAPABILITY: &str = "request.lifecycle.v1";
pub const AUTH_CAPABILITY: &str = "request.lifecycle.auth";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    BeforeSend,
    AfterHeaders,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderPatch {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
}

/// credentials 仅向显式声明 AUTH_CAPABILITY 的可信插件提供。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestEvent {
    pub metadata: Value,
    pub request_id: Value,
    pub stage: Stage,
    pub attempt: u32,
    pub headers: BTreeMap<String, String>,
    pub response: Option<Response>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// completed 表示 HTTP 响应传输完成，不表示模型或响应正文中的业务任务成功。
/// 终态通知仅用于清理，返回值不影响已确定的请求结果。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalEvent {
    pub metadata: Value,
    pub request_id: Value,
    pub stage: Stage,
    pub attempt: u32,
    #[serde(default)]
    pub token: Option<String>,
    pub status: Option<u16>,
    pub code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    Continue {
        #[serde(default)]
        headers: Vec<HeaderPatch>,
    },
    Wait {
        token: String,
        #[serde(
            default,
            rename = "pollAfterMs",
            skip_serializing_if = "Option::is_none"
        )]
        poll_after_ms: Option<u64>,
    },
    Retry {
        #[serde(default)]
        headers: Vec<HeaderPatch>,
    },
    Abort {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}
