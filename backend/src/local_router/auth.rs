use super::*;

pub(crate) struct OfficialUpstreamAuth {
    pub(crate) authorization: String,
    pub(crate) account_id: Option<String>,
}

pub(crate) fn incoming_chatgpt_account_id(request: &HttpRequest) -> Option<String> {
    incoming_header(request, CHATGPT_ACCOUNT_ID_HEADER)
        .map(str::trim)
        .filter(|account_id| !account_id.is_empty())
        .map(ToString::to_string)
}

pub(crate) async fn resolve_official_upstream_auth(
    request: &HttpRequest,
    router_bearer_token: &str,
    auth_path: &Path,
    auth_caches: &Mutex<crate::account_usage::OfficialAuthCaches>,
    // Only the account Codex itself is logged in as may reuse the token the
    // downstream request carries. Every other account reads its own document,
    // so one conversation never spends another account's login.
    accepts_incoming_authorization: bool,
) -> Option<OfficialUpstreamAuth> {
    if accepts_incoming_authorization
        && let Some(authorization) = incoming_openai_authorization(request, router_bearer_token)
    {
        let account_id = match incoming_chatgpt_account_id(request) {
            Some(account_id) => Some(account_id),
            None => read_cached_official_auth(auth_path, auth_caches)
                .await
                .and_then(|auth| auth.account_id),
        };
        return Some(OfficialUpstreamAuth {
            authorization: authorization.to_string(),
            account_id,
        });
    }

    let auth = read_cached_official_auth(auth_path, auth_caches).await?;
    Some(OfficialUpstreamAuth {
        authorization: format!("Bearer {}", auth.access_token),
        // The account ID stored with the selected OAuth token is authoritative.
        // An incoming value may have been captured by a long-lived downstream
        // WebSocket before the user switched accounts.
        account_id: auth
            .account_id
            .or_else(|| incoming_chatgpt_account_id(request)),
    })
}

pub(crate) async fn read_cached_official_auth(
    auth_path: &Path,
    auth_caches: &Mutex<crate::account_usage::OfficialAuthCaches>,
) -> Option<crate::account_usage::OfficialAuth> {
    let now = Instant::now();
    if let Some(cached) = auth_caches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .for_path(auth_path)
        .get(now)
    {
        return cached.ok();
    }

    let read_path = auth_path.to_path_buf();
    let result =
        tokio::task::spawn_blocking(move || crate::account_usage::read_official_auth(&read_path))
            .await
            .ok()?;
    let now = Instant::now();
    let mut caches = auth_caches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cache = caches.for_path(auth_path);
    if let Some(cached) = cache.get(now) {
        return cached.ok();
    }
    cache.store(result, now).ok()
}

pub(crate) fn route_display_name(route: &RouteTarget) -> &str {
    let route_name = route.route_name.trim();
    if route_name.is_empty() {
        route.provider_id.as_str()
    } else {
        route_name
    }
}

pub(crate) fn upstream_authority(base_url: &str) -> String {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return "已配置地址".to_string();
    };
    let Some(host) = url.host_str() else {
        return "已配置地址".to_string();
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}
