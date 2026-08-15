use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode, header::ACCEPT};
use serde::Serialize;
use serde_json::Value;

static LAST_GOOD_USAGE_ENDPOINT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

const USAGE_ENDPOINTS: [&str; 2] = [
    "https://chatgpt.com/backend-api/wham/usage",
    "https://chatgpt.com/backend-api/api/codex/usage",
];
const ACCOUNT_USAGE_CACHE_TTL: Duration = Duration::from_secs(30);
const ACCOUNT_USAGE_FAILURE_BACKOFF_INITIAL: Duration = Duration::from_secs(60);
const ACCOUNT_USAGE_FAILURE_BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);
const ACCOUNT_USAGE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
struct OfficialAuth {
    access_token: String,
    account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<AccountUsageWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<AccountUsageWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<AccountCredits>,
    pub fetched_at: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageWindow {
    pub used_percent: f64,
    pub window_minutes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountCredits {
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,
}

#[derive(Debug, Default)]
pub struct AccountUsageCache {
    snapshot: Option<AccountUsageSnapshot>,
    expires_at: Option<Instant>,
    consecutive_failures: u32,
    retry_at: Option<Instant>,
    last_error: Option<String>,
}

impl AccountUsageCache {
    pub async fn fetch(&mut self, codex_home: &Path) -> Result<AccountUsageSnapshot> {
        if let Some(cached) = self.cached_result(Instant::now()) {
            return cached.map_err(anyhow::Error::msg);
        }

        // reqwest snapshots the current system proxy when a client is built. Rebuild the
        // dedicated usage client for each network refresh so proxy changes do not require
        // restarting Codey. Cached results still avoid unnecessary requests and rebuilds.
        let result = match account_usage_http_client() {
            Ok(client) => fetch_official_account_usage(&client, codex_home).await,
            Err(error) => Err(error),
        };

        match result {
            Ok(snapshot) => {
                self.record_success(snapshot.clone(), Instant::now());
                Ok(snapshot)
            }
            Err(error) => {
                self.record_failure(error.to_string(), Instant::now());
                Err(error)
            }
        }
    }

    fn cached_result(
        &self,
        now: Instant,
    ) -> Option<std::result::Result<AccountUsageSnapshot, String>> {
        if self.expires_at.is_some_and(|expires_at| now < expires_at) {
            return self.snapshot.clone().map(Ok);
        }
        if self.retry_at.is_some_and(|retry_at| now < retry_at) {
            return Some(Err(self.last_error.clone().unwrap_or_else(|| {
                "官方额度暂时无法更新，稍后自动重试".to_string()
            })));
        }
        None
    }

    fn record_success(&mut self, snapshot: AccountUsageSnapshot, now: Instant) {
        self.snapshot = Some(snapshot);
        self.expires_at = Some(now + ACCOUNT_USAGE_CACHE_TTL);
        self.consecutive_failures = 0;
        self.retry_at = None;
        self.last_error = None;
    }

    fn record_failure(&mut self, error: String, now: Instant) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.retry_at = Some(now + account_usage_failure_backoff(self.consecutive_failures));
        self.last_error = Some(error);
    }
}

fn account_usage_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("Codey/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(ACCOUNT_USAGE_CONNECT_TIMEOUT)
        .build()
        .context("创建官方额度网络客户端失败")
}

fn account_usage_failure_backoff(consecutive_failures: u32) -> Duration {
    let shift = consecutive_failures.saturating_sub(1).min(3);
    let seconds = ACCOUNT_USAGE_FAILURE_BACKOFF_INITIAL
        .as_secs()
        .saturating_mul(1_u64 << shift)
        .min(ACCOUNT_USAGE_FAILURE_BACKOFF_MAX.as_secs());
    Duration::from_secs(seconds)
}

pub async fn fetch_official_account_usage(
    client: &Client,
    codex_home: &Path,
) -> Result<AccountUsageSnapshot> {
    let auth_path = codex_home.join("auth.json");
    let auth = tokio::task::spawn_blocking(move || read_official_auth(&auth_path))
        .await
        .context("读取 Codex 官方登录信息任务异常退出")??;
    let mut last_error = None;

    // 从上次成功的端点开始轮询，失败仍会回退到完整列表，结果不变但稳定
    // 状态下每次刷新只发一个请求。
    let start = LAST_GOOD_USAGE_ENDPOINT.load(std::sync::atomic::Ordering::Relaxed);
    for offset in 0..USAGE_ENDPOINTS.len() {
        let index = (start + offset) % USAGE_ENDPOINTS.len();
        let endpoint = USAGE_ENDPOINTS[index];
        let mut request = client
            .get(endpoint)
            .timeout(Duration::from_secs(8))
            .header(ACCEPT, "application/json")
            .header("user-agent", "codex_cli_rs")
            .bearer_auth(&auth.access_token);
        if let Some(account_id) = auth.account_id.as_deref() {
            request = request.header("chatgpt-account-id", account_id);
        }

        let response = request.send().await.with_context(|| "官方额度请求失败")?;
        let status = response.status();
        if matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ) {
            last_error = Some(format!("官方额度接口返回 {status}"));
            continue;
        }
        if !status.is_success() {
            bail!("官方额度接口返回 {status}");
        }

        let payload = response
            .json::<Value>()
            .await
            .with_context(|| "官方额度响应格式无效")?;
        LAST_GOOD_USAGE_ENDPOINT.store(index, std::sync::atomic::Ordering::Relaxed);
        return parse_account_usage(&payload, unix_timestamp());
    }

    bail!(
        "{}",
        last_error.unwrap_or_else(|| "未找到可用的官方额度接口".to_string())
    )
}

fn read_official_auth(path: &Path) -> Result<OfficialAuth> {
    let bytes = fs::read(path).with_context(|| "未找到 Codex 官方登录信息")?;
    let value: Value = serde_json::from_slice(&bytes).with_context(|| "Codex 登录信息格式无效")?;
    let is_chatgpt = value
        .get("auth_mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("chatgpt"));
    if !is_chatgpt {
        bail!("当前不是 ChatGPT 官方账号登录");
    }

    let tokens = value
        .get("tokens")
        .and_then(Value::as_object)
        .context("Codex 官方登录令牌缺失")?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .context("Codex 官方访问令牌缺失")?
        .to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|account_id| !account_id.is_empty())
        .map(ToString::to_string);

    Ok(OfficialAuth {
        access_token,
        account_id,
    })
}

fn parse_account_usage(value: &Value, fetched_at: u64) -> Result<AccountUsageSnapshot> {
    let snapshot = [
        "rate_limits",
        "rateLimits",
        "rate_limit",
        "rateLimit",
        "rate_limit_status",
        "rateLimitStatus",
    ]
    .iter()
    .find_map(|key| value.get(*key))
    .unwrap_or(value);

    let primary = parse_window(
        snapshot,
        &["primary", "primary_window", "primaryWindow"],
        fetched_at,
    );
    let secondary = parse_window(
        snapshot,
        &["secondary", "secondary_window", "secondaryWindow"],
        fetched_at,
    );
    let credits = parse_credits(snapshot).or_else(|| parse_credits(value));
    if primary.is_none() && secondary.is_none() && credits.is_none() {
        bail!("官方额度响应中没有可展示的额度信息");
    }

    let plan_type = string_field(value, &["plan_type", "planType"])
        .or_else(|| string_field(snapshot, &["plan_type", "planType"]));
    Ok(AccountUsageSnapshot {
        plan_type,
        primary,
        secondary,
        credits,
        fetched_at,
    })
}

fn parse_window(value: &Value, keys: &[&str], fetched_at: u64) -> Option<AccountUsageWindow> {
    let window = keys.iter().find_map(|key| value.get(*key))?;
    let used_percent = number_field(window, &["used_percent", "usedPercent"]).or_else(|| {
        number_field(window, &["remaining_percent", "remainingPercent"])
            .map(|remaining| 100.0 - remaining)
    })?;
    let window_minutes = u64_field(window, &["window_minutes", "windowMinutes"]).or_else(|| {
        u64_field(window, &["limit_window_seconds", "limitWindowSeconds"])
            .map(|seconds| seconds.div_ceil(60))
    })?;
    let resets_at = u64_field(window, &["resets_at", "resetsAt", "reset_at", "resetAt"])
        .map(normalize_timestamp)
        .or_else(|| {
            u64_field(window, &["reset_after_seconds", "resetAfterSeconds"])
                .map(|seconds| fetched_at.saturating_add(seconds))
        });

    Some(AccountUsageWindow {
        used_percent: used_percent.clamp(0.0, 100.0),
        window_minutes,
        resets_at,
    })
}

fn parse_credits(value: &Value) -> Option<AccountCredits> {
    let credits = value.get("credits")?;
    let unlimited = bool_field(credits, &["unlimited"]).unwrap_or(false);
    let balance = string_or_number_field(credits, &["balance"]);
    let has_credits = bool_field(credits, &["has_credits", "hasCredits"])
        .unwrap_or(unlimited || balance.as_deref().is_some_and(|balance| balance != "0"));
    Some(AccountCredits {
        has_credits,
        unlimited,
        balance,
    })
}

fn number_field(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|field| field.as_f64().or_else(|| field.as_str()?.parse().ok()))
    })
}

fn u64_field(value: &Value, keys: &[&str]) -> Option<u64> {
    number_field(value, keys)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| value.round() as u64)
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

fn string_or_number_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let field = value.get(*key)?;
        if let Some(value) = field
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_string());
        }
        field
            .as_f64()
            .filter(|value| value.is_finite())
            .map(|value| value.to_string())
    })
}

fn normalize_timestamp(timestamp: u64) -> u64 {
    if timestamp > 10_000_000_000 {
        timestamp / 1000
    } else {
        timestamp
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> AccountUsageSnapshot {
        AccountUsageSnapshot {
            plan_type: Some("plus".to_string()),
            primary: None,
            secondary: None,
            credits: Some(AccountCredits {
                has_credits: true,
                unlimited: false,
                balance: Some("10".to_string()),
            }),
            fetched_at: 1_700_000_000,
        }
    }

    #[test]
    fn successful_snapshots_are_reused_only_within_the_ttl() {
        let started_at = Instant::now();
        let snapshot = sample_snapshot();
        let mut cache = AccountUsageCache::default();
        cache.record_success(snapshot.clone(), started_at);

        assert_eq!(
            cache
                .cached_result(started_at + ACCOUNT_USAGE_CACHE_TTL - Duration::from_millis(1))
                .unwrap()
                .unwrap(),
            snapshot
        );
        assert!(
            cache
                .cached_result(started_at + ACCOUNT_USAGE_CACHE_TTL)
                .is_none()
        );
    }

    #[test]
    fn failures_back_off_exponentially_and_success_resets_the_delay() {
        let mut cache = AccountUsageCache::default();
        let mut attempt_at = Instant::now();
        for expected_seconds in [60, 120, 240, 300, 300] {
            cache.record_failure("offline".to_string(), attempt_at);
            assert_eq!(
                cache.retry_at.unwrap().duration_since(attempt_at),
                Duration::from_secs(expected_seconds)
            );
            assert_eq!(
                cache
                    .cached_result(attempt_at + Duration::from_secs(expected_seconds - 1))
                    .unwrap()
                    .unwrap_err(),
                "offline"
            );
            attempt_at += Duration::from_secs(expected_seconds);
        }

        cache.record_success(sample_snapshot(), attempt_at);
        assert_eq!(cache.consecutive_failures, 0);
        assert!(cache.retry_at.is_none());
        assert!(cache.last_error.is_none());
    }

    #[test]
    fn reads_chatgpt_auth_without_exposing_other_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.json");
        fs::write(
            &path,
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"token-value","account_id":"account-value","refresh_token":"do-not-copy"}}"#,
        )
        .unwrap();

        assert_eq!(
            read_official_auth(&path).unwrap(),
            OfficialAuth {
                access_token: "token-value".into(),
                account_id: Some("account-value".into()),
            }
        );
    }

    #[test]
    fn parses_official_rate_limit_snapshot() {
        let value = serde_json::json!({
            "rate_limits": {
                "primary": {
                    "used_percent": 15.0,
                    "window_minutes": 300,
                    "resets_at": 1_800_000_000
                },
                "secondary": {
                    "used_percent": 33.0,
                    "window_minutes": 10_080,
                    "resets_at": 1_800_500_000
                },
                "credits": {
                    "has_credits": false,
                    "unlimited": false,
                    "balance": "0"
                },
                "plan_type": "pro"
            }
        });

        let snapshot = parse_account_usage(&value, 1_700_000_000).unwrap();
        assert_eq!(snapshot.plan_type.as_deref(), Some("pro"));
        assert_eq!(snapshot.primary.unwrap().used_percent, 15.0);
        assert_eq!(snapshot.secondary.unwrap().window_minutes, 10_080);
        assert_eq!(snapshot.credits.unwrap().balance.as_deref(), Some("0"));
    }

    #[test]
    fn parses_rate_limit_status_detail_windows() {
        let value = serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 42.5,
                    "limit_window_seconds": 18_000,
                    "reset_after_seconds": 600
                },
                "secondary_window": {
                    "remaining_percent": 72,
                    "limit_window_seconds": 604_800,
                    "reset_after_seconds": 86_400
                }
            }
        });

        let snapshot = parse_account_usage(&value, 1_700_000_000).unwrap();
        assert_eq!(snapshot.primary.unwrap().window_minutes, 300);
        let secondary = snapshot.secondary.unwrap();
        assert_eq!(secondary.used_percent, 28.0);
        assert_eq!(secondary.resets_at, Some(1_700_086_400));
    }
}
