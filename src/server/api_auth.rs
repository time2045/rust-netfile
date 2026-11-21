use crate::auth;
use crate::config;
use crate::error::{ApiError, ApiResult};
use crate::server::{login_path, shared};
use crate::state::SharedState;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Deserialize, Default)]
struct LoginBody {
    #[serde(default)]
    password: String,
}

fn with_cookie(mut response: Response, cookie: String) -> Response {
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

fn too_many(message: String, retry_after: u64) -> ApiError {
    ApiError::new(StatusCode::TOO_MANY_REQUESTS, message)
        .with_extra(json!({ "retry_after": retry_after }))
}

pub async fn login(
    state: State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let state = shared(state);
    if state.is_locked() {
        return Err(ApiError::locked());
    }
    let ip = auth::client_ip(&headers, &addr);
    if let Some(retry) = state.limiter.retry_after(&ip) {
        return Err(too_many(
            format!("尝试次数过多，请在 {retry} 秒后重试"),
            retry,
        ));
    }
    let data: LoginBody = crate::server::parse_json(&body)?;
    let cfg = state.cfg();
    let secure = auth::request_is_secure(&headers, &addr);
    let prefix = auth::forwarded_prefix(&headers, &addr);
    if !cfg.has_password() {
        return Ok(
            Json(json!({ "success": true, "redirect": crate::server::root_path(&prefix) }))
                .into_response(),
        );
    }
    let hash = cfg.password_hash.clone().unwrap_or_default();
    let password = data.password;
    let verified = tokio::task::spawn_blocking(move || auth::verify_password(&hash, &password))
        .await
        .unwrap_or(false);
    if !verified {
        state.log.line(&format!("{ip} 登录失败：密码错误"));
        if let Some(retry) =
            state
                .limiter
                .record_failure(&ip, cfg.max_login_failures, cfg.login_block_minutes)
        {
            state
                .log
                .line(&format!("{ip} 连续登录失败，已临时封禁 {retry} 秒"));
            return Err(too_many(
                format!("密码错误次数过多，请在 {retry} 秒后重试"),
                retry,
            ));
        }
        return Err(ApiError::unauthorized("密码错误，请重新输入。"));
    }
    state.limiter.reset(&ip);
    let session = auth::create_session_value(&cfg.secret, &cfg.auth_epoch, cfg.session_hours);
    let cookie = auth::session_cookie(&session, cfg.session_hours * 3600, secure);
    state.log.line(&format!("{ip} 登录成功"));
    let response = Json(json!({
        "success": true,
        "redirect": crate::server::root_path(&prefix),
    }))
    .into_response();
    Ok(with_cookie(response, cookie))
}

pub async fn logout(
    state: State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let state = shared(state);
    let ip = auth::client_ip(&headers, &addr);
    let secure = auth::request_is_secure(&headers, &addr);
    state.log.line(&format!("{ip} 退出登录"));
    let response = Json(json!({ "success": true })).into_response();
    with_cookie(response, auth::clear_session_cookie(secure))
}

pub async fn ping(state: State<SharedState>) -> Json<serde_json::Value> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    Json(json!({
        "success": true,
        "t": now,
        "locked": state.is_locked(),
        "version": config::APP_VERSION,
    }))
}

pub fn redirect_login(prefix: &str) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    if let Ok(value) = HeaderValue::from_str(&login_path(prefix)) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}
