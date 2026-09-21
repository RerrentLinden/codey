use super::*;

pub(crate) struct OfficialUpstreamAuth {
    pub(crate) authorization: String,
    pub(crate) account_id: Option<String>,
}

struct IncomingTokenIatCache {
    token: String,
    issued_at: Option<u64>,
}

/// Codex 在同一登录期内会反复发送同一条请求头令牌；只保留最近一次解码结果。
static LAST_INCOMING_TOKEN_IAT: Mutex<Option<IncomingTokenIatCache>> = Mutex::new(None);

pub(crate) fn incoming_chatgpt_account_id(request: &HttpRequest) -> Option<String> {
    incoming_header(request, CHATGPT_ACCOUNT_ID_HEADER)
        .map(str::trim)
        .filter(|account_id| !account_id.is_empty())
        .map(ToString::to_string)
}

/// 取出 Bearer 方案携带的令牌，其他方案或空值返回 None。
fn bearer_token(authorization: &str) -> Option<&str> {
    let (scheme, token) = authorization.trim().split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

fn access_token_issued_at(token: &str) -> Option<u64> {
    crate::official_accounts::access_token_issued_at(token)
}

fn incoming_access_token_issued_at(token: &str) -> Option<u64> {
    let mut cache = LAST_INCOMING_TOKEN_IAT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(cached) = cache.as_ref()
        && cached.token == token
    {
        return cached.issued_at;
    }
    let issued_at = access_token_issued_at(token);
    *cache = Some(IncomingTokenIatCache {
        token: token.to_string(),
        issued_at,
    });
    issued_at
}

/// 已启动的 Codex 进程会把登录时拿到的令牌留在内存里；用户在 Codey 里重新
/// 登录后账号文件已经换成新令牌，请求头仍会带着旧令牌。两份令牌都能读出签发
/// 时间时取较新的一份；请求头不是可解读的 JWT 而账号文件有令牌时也以账号
/// 文件为准，官方端点只接受这种登录令牌，避免把已经失效的旧令牌继续转发。
#[cfg(test)]
pub(crate) fn stored_token_supersedes_incoming(
    incoming_authorization: &str,
    stored_access_token: &str,
) -> bool {
    stored_token_supersedes_incoming_issued_at(
        incoming_authorization,
        stored_access_token,
        access_token_issued_at(stored_access_token),
    )
}

fn stored_auth_supersedes_incoming(
    incoming_authorization: &str,
    stored: &crate::account_usage::OfficialAuth,
) -> bool {
    stored_token_supersedes_incoming_issued_at(
        incoming_authorization,
        &stored.access_token,
        stored.issued_at,
    )
}

fn stored_token_supersedes_incoming_issued_at(
    incoming_authorization: &str,
    stored_access_token: &str,
    stored_issued_at: Option<u64>,
) -> bool {
    let incoming_token = bearer_token(incoming_authorization);
    if incoming_token == Some(stored_access_token) {
        return false;
    }
    let incoming_issued_at = incoming_token.and_then(incoming_access_token_issued_at);
    match (incoming_issued_at, stored_issued_at) {
        (Some(incoming), Some(stored)) => stored > incoming,
        (None, Some(_)) => true,
        _ => false,
    }
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
        // 重新登录只替换账号文件，已经启动的 Codex 仍会发送登录时缓存的旧
        // 令牌；账号文件里的令牌更新时以它为准，新登录才能立即生效。
        let stored = read_cached_official_auth(auth_path, auth_caches).await;
        if let Some(stored_auth) = stored.as_ref()
            && stored_auth_supersedes_incoming(authorization, stored_auth)
        {
            return Some(OfficialUpstreamAuth {
                authorization: format!("Bearer {}", stored_auth.access_token),
                account_id: stored_auth
                    .account_id
                    .clone()
                    .or_else(|| incoming_chatgpt_account_id(request)),
            });
        }
        let account_id = match incoming_chatgpt_account_id(request) {
            Some(account_id) => Some(account_id),
            None => stored.and_then(|auth| auth.account_id.clone()),
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
            .clone()
            .or_else(|| incoming_chatgpt_account_id(request)),
    })
}

pub(crate) async fn read_cached_official_auth(
    auth_path: &Path,
    auth_caches: &Mutex<crate::account_usage::OfficialAuthCaches>,
) -> Option<std::sync::Arc<crate::account_usage::OfficialAuth>> {
    let now = Instant::now();
    if let Some(cached) = auth_caches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .for_path(auth_path)
        .get(auth_path, now)
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
    if let Some(cached) = cache.get(auth_path, now) {
        return cached.ok();
    }
    cache.store(auth_path, result, now).ok()
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
