use crate::auth::{self, client_ip, forwarded_prefix};
use crate::error::ApiError;
use crate::state::{AppState, SharedState};
use axum::extract::{ConnectInfo, Path as AxumPath, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

pub mod api_auth;
pub mod api_download;
pub mod api_files;
pub mod api_list;
pub mod api_speed;
pub mod api_upload;
pub mod assets;

pub fn build_router(state: SharedState) -> Router {
    let public = Router::new()
        .route("/login", get(login_page))
        .route("/api/login", post(api_auth::login))
        .route("/api/ping", get(api_auth::ping))
        .route("/config.js", get(config_js))
        .route("/static/{*file}", get(static_file))
        .route("/favicon.ico", get(favicon));

    let protected = Router::new()
        .route("/", get(index_page))
        .route("/api/logout", post(api_auth::logout))
        .route("/api/list", get(api_list::list))
        .route("/api/folders", get(api_list::folders))
        .route("/api/download", get(api_download::download))
        .route("/api/preview", get(api_download::preview))
        .route("/api/download_folder", post(api_download::download_folder))
        .route("/api/download_many", post(api_download::download_many))
        .route("/api/download_file", get(api_download::download_file))
        .route("/api/task/status", get(api_download::task_status))
        .route("/api/task/cleanup", post(api_download::task_cleanup))
        .route("/api/mkdir", post(api_files::mkdir))
        .route("/api/rename", post(api_files::rename))
        .route("/api/move", post(api_files::move_one))
        .route("/api/move_many", post(api_files::move_many))
        .route("/api/copy", post(api_files::copy))
        .route("/api/copy_many", post(api_files::copy_many))
        .route("/api/delete", post(api_files::delete))
        .route("/api/delete_many", post(api_files::delete_many))
        .route("/api/check_conflicts", post(api_files::check_conflicts))
        .route("/api/upload/status", get(api_upload::status))
        .route("/api/upload/chunk", post(api_upload::chunk))
        .route("/api/upload/complete", post(api_upload::complete))
        .route("/api/upload/abort", post(api_upload::abort))
        .route("/api/speed/down", get(api_speed::down))
        .route("/api/speed/up", post(api_speed::up))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .merge(public)
        .merge(protected)
        .fallback(fallback)
        .layer(middleware::from_fn_with_state(state.clone(), csrf_guard))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            track_requests,
        ))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

fn redirect_to(location: &str) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

pub fn root_path(prefix: &str) -> String {
    if prefix.is_empty() {
        "/".to_string()
    } else {
        format!("{prefix}/")
    }
}

pub fn login_path(prefix: &str) -> String {
    if prefix.is_empty() {
        "/login".to_string()
    } else {
        format!("{prefix}/login")
    }
}

fn is_asset_request(path: &str) -> bool {
    path.starts_with("/static/") || path == "/favicon.ico" || path == "/config.js"
}

fn skip_access_log(path: &str) -> bool {
    is_asset_request(path)
        || path == "/api/ping"
        || path == "/api/task/status"
        || path == "/api/folders"
        || path == "/api/upload/status"
        || path == "/api/upload/chunk"
        || path.starts_with("/api/speed/")
}

async fn security_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers
        .entry(header::X_FRAME_OPTIONS)
        .or_insert(HeaderValue::from_static("DENY"));
    headers
        .entry(header::X_CONTENT_TYPE_OPTIONS)
        .or_insert(HeaderValue::from_static("nosniff"));
    headers
        .entry(header::REFERRER_POLICY)
        .or_insert(HeaderValue::from_static("same-origin"));
    headers
        .entry(header::HeaderName::from_static("permissions-policy"))
        .or_insert(HeaderValue::from_static(
            "camera=(), microphone=(), geolocation=()",
        ));
    let is_html = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("text/html"))
        .unwrap_or(false);
    if is_html {
        headers.entry(header::HeaderName::from_static("content-security-policy")).or_insert(HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
        ));
        headers
            .entry(header::CACHE_CONTROL)
            .or_insert(HeaderValue::from_static("no-store"));
    } else if path.starts_with("/api/") {
        headers
            .entry(header::CACHE_CONTROL)
            .or_insert(HeaderValue::from_static("no-store"));
        headers
            .entry(header::HeaderName::from_static("content-security-policy"))
            .or_insert(HeaderValue::from_static("default-src 'none'"));
    } else if path.starts_with("/static/") {
        headers
            .entry(header::CACHE_CONTROL)
            .or_insert(HeaderValue::from_static("no-cache"));
    }
    response
}

async fn track_requests(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let ip = client_ip(req.headers(), &addr);
    state.stats.request_start();
    state.stats.touch_client(&ip);
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let response = next.run(req).await;
    state.stats.request_end();
    if !skip_access_log(&path) {
        let mut logged_path = path;
        if !query.is_empty() {
            logged_path.push('?');
            logged_path.push_str(&query.chars().take(300).collect::<String>());
        }
        state.log.access(
            &ip,
            method.as_str(),
            &logged_path,
            response.status().as_u16(),
            started.elapsed().as_millis() as u64,
        );
    }
    response
}

async fn csrf_guard(
    State(_state): State<SharedState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let headers = req.headers().clone();
    let had_csrf = auth::cookie_value(&headers, auth::CSRF_COOKIE).is_some();
    if auth::is_mutating(req.method()) && !auth::csrf_matches(&headers) {
        return Err(ApiError::forbidden("页面已过期，请刷新后重试"));
    }
    let mut response = next.run(req).await;
    if !had_csrf {
        let token = crate::config::random_hex(16);
        if let Ok(value) = HeaderValue::from_str(&auth::csrf_cookie(&token)) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    Ok(response)
}

async fn require_auth(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let path = req.uri().path().to_string();
    let is_api = path.starts_with("/api/");
    let prefix = forwarded_prefix(req.headers(), &addr);
    if state.is_locked() {
        if is_api {
            return Err(ApiError::locked());
        }
        return Ok(redirect_to(&login_path(&prefix)));
    }
    let cfg = state.cfg();
    if !cfg.has_password() {
        return Ok(next.run(req).await);
    }
    let session_ok = auth::cookie_value(req.headers(), auth::SESSION_COOKIE)
        .and_then(|value| auth::verify_session_value(&cfg.secret, &cfg.auth_epoch, &value))
        .is_some();
    if session_ok {
        return Ok(next.run(req).await);
    }
    if is_api {
        Err(ApiError::unauthorized("未登录或登录已失效"))
    } else {
        Ok(redirect_to(&login_path(&prefix)))
    }
}

async fn fallback(req: Request) -> Response {
    if req.uri().path().starts_with("/api/") {
        ApiError::not_found("接口不存在").into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "页面不存在",
        )
            .into_response()
    }
}

async fn index_page(State(state): State<SharedState>) -> Response {
    assets::html_response(assets::INDEX_HTML, &state)
}

async fn login_page(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let cfg = state.cfg();
    let prefix = forwarded_prefix(req.headers(), &addr);
    let authed = auth::cookie_value(req.headers(), auth::SESSION_COOKIE)
        .and_then(|value| auth::verify_session_value(&cfg.secret, &cfg.auth_epoch, &value))
        .is_some();
    if !cfg.has_password() || authed {
        if state.is_locked() {
            return assets::html_response(assets::LOGIN_HTML, &state);
        }
        return redirect_to(&root_path(&prefix));
    }
    assets::html_response(assets::LOGIN_HTML, &state)
}

async fn config_js(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let prefix = forwarded_prefix(req.headers(), &addr);
    let cfg = state.cfg();
    let body = serde_json::json!({
        "root": prefix,
        "version": crate::config::APP_VERSION,
        "locked": state.is_locked(),
        "has_password": cfg.has_password(),
        "conflict_mode": cfg.conflict_mode.as_str(),
        "upload_limit_bytes": crate::config::upload_limit_bytes(&cfg),
    })
    .to_string();
    let script = format!("window.NB_APP = {body};");
    let mut response = script.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/javascript; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn static_file(AxumPath(file): AxumPath<String>) -> Response {
    match assets::static_asset(&file) {
        Some((mime, bytes)) => {
            let mut response = bytes.into_response();
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
            response
        }
        None => ApiError::not_found("资源不存在").into_response(),
    }
}

async fn favicon() -> Response {
    let mut response = assets::FAVICON.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/x-icon"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    response
}

pub fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|e| ApiError::bad(format!("请求数据格式错误：{e}")))
}

pub fn json_value(body: &[u8]) -> Result<serde_json::Value, ApiError> {
    parse_json(body)
}

pub fn get_str(data: &serde_json::Value, key: &str) -> Result<String, ApiError> {
    data.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ApiError::bad(format!("缺少必要参数：{key}")))
}

pub fn get_str_opt(data: &serde_json::Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn get_u32(data: &serde_json::Value, key: &str, default: u32) -> u32 {
    data.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(default)
}

pub fn remote_client_ip(headers: &axum::http::HeaderMap, addr: &SocketAddr) -> String {
    client_ip(headers, addr)
}

pub fn shared(state: State<SharedState>) -> Arc<AppState> {
    state.0.clone()
}
