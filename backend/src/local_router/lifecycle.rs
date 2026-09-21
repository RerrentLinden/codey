use super::responses::format_upstream_headers;
use super::*;
use crate::codey_plugins::lifecycle::{
    LifecycleDecision, LifecycleError, LifecycleOutcome, LifecycleRequest, LifecycleResponse,
    LifecycleStage, has_plugins,
};

pub(super) fn apply_codey_plugin_header_patches(
    headers: &mut HeaderMap,
    patches: Vec<crate::codey_plugins::HeaderPatch>,
) {
    let parsed = patches
        .into_iter()
        .map(|patch| {
            if !crate::codey_plugins::allowed_header_name(&patch.name) {
                return None;
            }
            let name = HeaderName::from_bytes(patch.name.as_bytes()).ok()?;
            let value = match patch.value {
                Some(value) => Some(HeaderValue::from_str(&value).ok()?),
                None => None,
            };
            Some((name, value))
        })
        .collect::<Option<Vec<_>>>();
    let Some(parsed) = parsed else { return };
    for (name, value) in parsed {
        if let Some(value) = value {
            headers.insert(name, value);
        } else {
            headers.remove(name);
        }
    }
}

pub(super) fn lifecycle_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_owned()))
        })
        .collect()
}

pub(super) fn request_lifecycle(
    headers: &HeaderMap,
    resolved: &RouteSelection,
    bridge: ProtocolBridge,
    request_kind: ResponsesRequestKind,
    stream: bool,
    subagent: bool,
) -> LifecycleRequest {
    if !has_plugins() {
        return LifecycleRequest::inert();
    }
    let upstream_account_id = headers
        .get(CHATGPT_ACCOUNT_ID_HEADER)
        .and_then(|v| v.to_str().ok());
    let access_token = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token.trim());
    // 已解析令牌中的套餐声明只作为可选元数据，不推测缺失的账号身份。
    let account_type = resolved
        .route
        .official_account
        .then(|| {
            access_token
                .and_then(crate::official_accounts::jwt_claims)
                .and_then(|claims| {
                    claims
                        .get("https://api.openai.com/auth")?
                        .get("chatgpt_plan_type")?
                        .as_str()
                        .map(str::to_owned)
                })
        })
        .flatten();
    let metadata = json!({
        "requestId": current_router_request_id().unwrap_or_else(|| Uuid::new_v4().to_string()),
        "routeId": resolved.provider_id,
        "officialAccountId": resolved.route.official_auth.as_ref().map(|auth| auth.account_id.as_str()),
        "officialAccountEmail": official_account_email(&resolved.route),
        "upstreamAccountId": upstream_account_id,
        "accountType": account_type,
        "requestedModel": resolved.requested_model,
        "model": resolved.upstream_model,
        "protocol": bridge.upstream_protocol().label(),
        "stream": stream,
        "requestKind": request_kind.label(),
        "subagent": subagent,
    });
    let credentials = resolved
        .route
        .official_account
        .then(|| {
            access_token.map(|token| {
                json!({
                    "accessToken": token,
                    "upstreamAccountId": upstream_account_id,
                })
            })
        })
        .flatten();
    LifecycleRequest::new(metadata, credentials)
}

fn official_account_email(route: &RouteTarget) -> Option<&str> {
    if !route.official_account {
        return None;
    }
    route.official_auth.as_ref()?.email.as_deref()
}

/// 生命周期只允许在尚未向下游写出响应时重发。与 reasoning 回退共享两次发送预算。
#[allow(clippy::too_many_arguments)]
pub(super) async fn send_lifecycle_http<D: ResponsesDownstream + ?Sized>(
    downstream: &mut D,
    lifecycle: &mut LifecycleRequest,
    client: &reqwest::Client,
    url: &str,
    headers: &mut HeaderMap,
    body: Bytes,
    attempt: &mut u32,
    timeout: Duration,
) -> Result<
    std::result::Result<
        std::result::Result<reqwest::Response, reqwest::Error>,
        tokio::time::error::Elapsed,
    >,
> {
    loop {
        let decision = await_upstream(
            downstream,
            lifecycle.dispatch(
                LifecycleStage::BeforeSend,
                *attempt,
                lifecycle_headers(headers),
                None,
            ),
        )
        .await??;
        match decision {
            LifecycleDecision::Continue(patches) => {
                apply_codey_plugin_header_patches(headers, patches)
            }
            LifecycleDecision::Retry(_) => {
                return Err(LifecycleError {
                    status: 409,
                    code: "plugin_retry_invalid_stage".into(),
                    message: "请求发送前不能要求重发".into(),
                }
                .into());
            }
        }
        if let Some(probe) = downstream.request_log_probe() {
            probe.set_upstream_request_headers(&format_upstream_headers(headers));
        }
        let result = await_upstream(
            downstream,
            tokio::time::timeout(
                timeout,
                client
                    .post(url)
                    .headers(headers.clone())
                    .body(body.clone())
                    .send(),
            ),
        )
        .await?;
        let Ok(Ok(response)) = &result else {
            return Ok(result);
        };
        let decision = await_upstream(
            downstream,
            lifecycle.dispatch(
                LifecycleStage::AfterHeaders,
                *attempt,
                lifecycle_headers(headers),
                Some(LifecycleResponse {
                    status: response.status().as_u16(),
                    headers: lifecycle_headers(response.headers()),
                }),
            ),
        )
        .await??;
        match decision {
            LifecycleDecision::Continue(_) => return Ok(result),
            LifecycleDecision::Retry(patches) => {
                if *attempt >= 1 || downstream.event_stream_started() {
                    return Err(LifecycleError {
                        status: 409,
                        code: "plugin_retry_limit".into(),
                        message: "请求已达到重发上限或响应已经开始".into(),
                    }
                    .into());
                }
                drop(result);
                apply_codey_plugin_header_patches(headers, patches);
                *attempt += 1;
                if let Some(probe) = downstream.request_log_probe() {
                    probe.mark_fallback("plugin_request_retry");
                }
            }
        }
    }
}

/// 汇总写回的错误与传输结果，避免 HTTP 错误成功写回时被误报为请求完成。
pub(super) struct LifecycleDownstream<'a, D: ResponsesDownstream + ?Sized> {
    pub inner: &'a mut D,
    pub status: Option<u16>,
    pub error: Option<String>,
}

impl<D: ResponsesDownstream + ?Sized> LifecycleDownstream<'_, D> {
    pub fn finish(&self, lifecycle: &mut LifecycleRequest, result: &Result<()>) {
        let cancelled = result.as_ref().err().is_some_and(|error| {
            error.is::<DownstreamClosed>()
                || error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::NotConnected
                    )
                })
        });
        let outcome = if cancelled {
            LifecycleOutcome::Cancelled
        } else if result.is_err() || self.error.is_some() || self.status.is_some_and(|s| s >= 400) {
            LifecycleOutcome::Failed
        } else {
            LifecycleOutcome::Completed
        };
        let code = self.error.as_deref().or_else(|| {
            result.as_ref().err().map(|_| {
                if cancelled {
                    "downstream_closed"
                } else {
                    "upstream_response_failed"
                }
            })
        });
        lifecycle.finish(outcome, self.status, code);
    }
}

#[async_trait]
impl<D: ResponsesDownstream + ?Sized> ResponsesDownstream for LifecycleDownstream<'_, D> {
    async fn wait_for_upstream<T, F>(&mut self, future: F) -> Result<T>
    where
        T: Send,
        F: std::future::Future<Output = T> + Send,
    {
        self.inner.wait_for_upstream(future).await
    }
    fn is_websocket(&self) -> bool {
        self.inner.is_websocket()
    }
    fn event_stream_started(&self) -> bool {
        self.inner.event_stream_started()
    }
    fn request_log_probe(&self) -> Option<&RouteRequestLogProbe> {
        self.inner.request_log_probe()
    }
    fn select_route(&mut self, route: &RouteTarget) {
        self.inner.select_route(route);
    }
    fn prepare_native_http_fallback(
        &mut self,
        route: &RouteTarget,
        headers: &HeaderMap,
        body: &mut Value,
    ) -> Result<bool> {
        self.inner
            .prepare_native_http_fallback(route, headers, body)
    }
    fn prepare_adapted_response_context(&mut self, body: &mut Value) -> Result<bool> {
        self.inner.prepare_adapted_response_context(body)
    }
    fn remember_adapted_response(&mut self, id: &str, output: &[Value]) -> Result<()> {
        self.inner.remember_adapted_response(id, output)
    }
    async fn write_error(
        &mut self,
        status: u16,
        code: &str,
        message: String,
        route: Option<&RouteTarget>,
    ) -> Result<()> {
        self.status = Some(status);
        self.error = Some(code.into());
        self.inner.write_error(status, code, message, route).await
    }
    async fn write_text_error(&mut self, status: u16, code: &str, message: String) -> Result<()> {
        self.status = Some(status);
        self.error = Some(code.into());
        self.inner.write_text_error(status, code, message).await
    }
    async fn write_json(&mut self, status: u16, value: &Value) -> Result<()> {
        self.status = Some(status);
        self.inner.write_json(status, value).await
    }
    async fn start_event_stream(&mut self) -> Result<()> {
        self.status = Some(200);
        self.inner.start_event_stream().await
    }
    async fn write_event(&mut self, event: &Value) -> Result<()> {
        self.inner.write_event(event).await
    }
    async fn finish_event_stream(&mut self) -> Result<()> {
        self.inner.finish_event_stream().await
    }
    async fn proxy_response_with_probe(
        &mut self,
        response: reqwest::Response,
        probe: Option<&RouteRequestLogProbe>,
    ) -> Result<()> {
        self.status = Some(response.status().as_u16());
        self.inner.proxy_response_with_probe(response, probe).await
    }
    async fn try_proxy_upstream_websocket_with_probe(
        &mut self,
        route: &RouteTarget,
        headers: &HeaderMap,
        body: &mut Value,
        discard: bool,
        probe: Option<&RouteRequestLogProbe>,
    ) -> Result<UpstreamWebSocketAttempt> {
        self.inner
            .try_proxy_upstream_websocket_with_probe(route, headers, body, discard, probe)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_email_comes_only_from_official_route_metadata() {
        let (config, _, model) =
            super::super::tests::router_config("https://example.com/v1".into());
        let snapshot = RouterSnapshot::from_config(&config);
        let mut route = snapshot
            .target_for_model(&model)
            .unwrap()
            .route
            .as_ref()
            .clone();
        route.official_auth = Some(OfficialRouteAuth {
            account_id: "local-account".into(),
            email: Some("member@example.com".into()),
            path: PathBuf::from("/unused/auth.json"),
            accepts_incoming_authorization: false,
        });
        // 非官方线路即使意外携带账号元数据，也不能将邮箱交给插件。
        assert!(official_account_email(&route).is_none());
        route.official_account = true;
        assert_eq!(official_account_email(&route), Some("member@example.com"));
        route.official_auth.as_mut().unwrap().email = None;
        assert_eq!(json!(official_account_email(&route)), Value::Null);
        route.official_auth = None;
        assert_eq!(json!(official_account_email(&route)), Value::Null);
    }
}
