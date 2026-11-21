use crate::auth;
use crate::config::{AppConfig, ConflictMode, ZipCompress};
use crate::error::{ApiError, ApiResult};
use crate::recycle;
use crate::server::shared;
use crate::state::SharedState;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Serialize)]
struct ConfigView {
    port: u16,
    bind_lan: bool,
    root_dir: String,
    has_password: bool,
    upload_limit_mb: u64,
    conflict_mode: ConflictMode,
    zip_compress: ZipCompress,
    zip_include_folder: bool,
    recycle_days: u32,
    session_hours: u32,
    max_login_failures: u32,
    login_block_minutes: u32,
    auto_start: bool,
}

impl From<&AppConfig> for ConfigView {
    fn from(cfg: &AppConfig) -> Self {
        Self {
            port: cfg.port,
            bind_lan: cfg.bind_lan,
            root_dir: cfg.root_dir.clone(),
            has_password: cfg.has_password(),
            upload_limit_mb: cfg.upload_limit_mb,
            conflict_mode: cfg.conflict_mode,
            zip_compress: cfg.zip_compress,
            zip_include_folder: cfg.zip_include_folder,
            recycle_days: cfg.recycle_days,
            session_hours: cfg.session_hours,
            max_login_failures: cfg.max_login_failures,
            login_block_minutes: cfg.login_block_minutes,
            auto_start: cfg.auto_start,
        }
    }
}

pub async fn overview(state: State<SharedState>) -> Json<Value> {
    let state = shared(state);
    let cfg = state.cfg();
    Json(json!({
        "success": true,
        "snapshot": state.snapshot(),
        "config": ConfigView::from(&cfg),
    }))
}

pub async fn get_config(state: State<SharedState>) -> Json<Value> {
    let state = shared(state);
    let cfg = state.cfg();
    Json(json!({ "success": true, "config": ConfigView::from(&cfg) }))
}

#[derive(Deserialize, Default)]
struct ConfigUpdate {
    port: Option<u16>,
    bind_lan: Option<bool>,
    root_dir: Option<String>,
    upload_limit_mb: Option<u64>,
    conflict_mode: Option<ConflictMode>,
    zip_compress: Option<ZipCompress>,
    zip_include_folder: Option<bool>,
    recycle_days: Option<u32>,
    session_hours: Option<u32>,
    max_login_failures: Option<u32>,
    login_block_minutes: Option<u32>,
    auto_start: Option<bool>,
}

pub async fn save_config(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let update: ConfigUpdate = crate::server::parse_json(&body)?;
    let current = state.cfg();
    let old_port = state.bound_port.load(std::sync::atomic::Ordering::Relaxed);
    let old_bind = state
        .bound_bind_lan
        .load(std::sync::atomic::Ordering::Relaxed);
    let running = state.running.load(std::sync::atomic::Ordering::Relaxed);
    let mut next = current.clone();
    if let Some(port) = update.port {
        next.port = port;
    }
    if let Some(bind) = update.bind_lan {
        next.bind_lan = bind;
    }
    if let Some(root) = update.root_dir {
        next.root_dir = root;
    }
    if let Some(limit) = update.upload_limit_mb {
        next.upload_limit_mb = limit;
    }
    if let Some(mode) = update.conflict_mode {
        next.conflict_mode = mode;
    }
    if let Some(zip) = update.zip_compress {
        next.zip_compress = zip;
    }
    if let Some(include) = update.zip_include_folder {
        next.zip_include_folder = include;
    }
    if let Some(days) = update.recycle_days {
        next.recycle_days = days;
    }
    if let Some(hours) = update.session_hours {
        next.session_hours = hours;
    }
    if let Some(failures) = update.max_login_failures {
        next.max_login_failures = failures;
    }
    if let Some(minutes) = update.login_block_minutes {
        next.login_block_minutes = minutes;
    }
    if let Some(auto_start) = update.auto_start {
        next.auto_start = auto_start;
    }
    if let Err(err) = state.save_config(next.clone()) {
        return Err(ApiError::bad(err));
    }
    if running && (next.port != current.port || next.bind_lan != current.bind_lan) {
        let state2 = state.clone();
        let rollback_port = if old_port == 0 {
            current.port
        } else {
            old_port
        };
        let rollback_bind = old_bind;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            let _ = state2.restart_server(rollback_port, rollback_bind).await;
        });
        return Ok(Json(json!({
            "success": true,
            "restarting": true,
            "port": next.port,
            "message": "配置已保存，正在重启服务以应用端口/监听范围",
        })));
    }
    state.log.line("控制台已更新服务器配置");
    Ok(Json(
        json!({ "success": true, "restarting": false, "message": "配置已保存并生效" }),
    ))
}

#[derive(Deserialize, Default)]
struct PasswordUpdate {
    #[serde(default)]
    current: String,
    new_password: Option<String>,
    new: Option<String>,
}

pub async fn change_password(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let update: PasswordUpdate = crate::server::parse_json(&body)?;
    let new_password = update.new_password.or(update.new).unwrap_or_default();
    auth::validate_password(&new_password).map_err(ApiError::bad)?;
    let mut cfg = state.cfg();
    if cfg.has_password() {
        let hash = cfg.password_hash.clone().unwrap_or_default();
        let current = update.current;
        let ok = tokio::task::spawn_blocking(move || auth::verify_password(&hash, &current))
            .await
            .unwrap_or(false);
        if !ok {
            return Err(ApiError::bad("当前密码不正确"));
        }
    }
    let hashed = tokio::task::spawn_blocking(move || auth::hash_password(&new_password))
        .await
        .map_err(|e| ApiError::internal(format!("密码处理失败：{e}")))?
        .map_err(ApiError::internal)?;
    cfg.password_hash = Some(hashed);
    cfg.rotate_auth_epoch();
    if let Err(err) = state.save_config(cfg) {
        return Err(ApiError::bad(err));
    }
    state.log.line("访问密码已更新，全部登录会话已失效");
    Ok(Json(json!({
        "success": true,
        "relogin": true,
        "message": "密码已更新，请重新登录",
    })))
}

pub async fn lock(state: State<SharedState>) -> Json<Value> {
    let state = shared(state);
    state.set_locked(true);
    Json(json!({
        "success": true,
        "message": "服务已锁定。请在桌面启动台或系统托盘解锁（无桌面环境时重启服务）",
    }))
}

pub async fn restart(state: State<SharedState>) -> ApiResult<Json<Value>> {
    let state = shared(state);
    if !state.running.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(ApiError::bad("服务未运行"));
    }
    let old_port = state.bound_port.load(std::sync::atomic::Ordering::Relaxed);
    let old_bind = state
        .bound_bind_lan
        .load(std::sync::atomic::Ordering::Relaxed);
    let state2 = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        let _ = state2.restart_server(old_port, old_bind).await;
    });
    Ok(Json(json!({
        "success": true,
        "restarting": true,
        "message": "正在重启服务",
    })))
}

#[derive(Deserialize, Default)]
pub struct LogQuery {
    kind: Option<String>,
    page: Option<String>,
    page_size: Option<String>,
}

pub async fn logs(state: State<SharedState>, Query(query): Query<LogQuery>) -> Json<Value> {
    let state = shared(state);
    let kind = query.kind.unwrap_or_else(|| "access".into());
    let page = query
        .page
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);
    let page_size = query
        .page_size
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(10, 500);
    let value = match kind.as_str() {
        "upload" => serde_json::to_value(state.log.audit_page("upload", page, page_size)),
        "download" => serde_json::to_value(state.log.audit_page("download", page, page_size)),
        _ => serde_json::to_value(state.log.access_page(page, page_size)),
    }
    .unwrap_or_else(|_| json!({}));
    let mut response = value;
    if let Value::Object(map) = &mut response {
        map.insert("success".into(), Value::Bool(true));
        map.insert("kind".into(), Value::String(kind));
    }
    Json(response)
}

pub async fn clear_logs(state: State<SharedState>) -> Json<Value> {
    let state = shared(state);
    state.log.clear_all();
    Json(json!({ "success": true, "message": "日志已清除" }))
}

pub async fn recycle_list(state: State<SharedState>) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let state2 = state.clone();
    let entries = tokio::task::spawn_blocking(move || recycle::scan(&state2))
        .await
        .map_err(|e| ApiError::internal(format!("读取回收站失败：{e}")))?;
    Ok(Json(json!({
        "success": true,
        "entries": entries,
        "days": state.cfg().recycle_days,
    })))
}

#[derive(Deserialize, Default)]
struct NamesBody {
    #[serde(default)]
    names: Vec<String>,
}

fn result_json(ok: u64, fails: Vec<String>, verb: &str) -> Value {
    let message = if fails.is_empty() {
        format!("{verb}完成：共 {ok} 项")
    } else {
        format!("{verb}完成：成功 {ok} 项，失败 {} 项", fails.len())
    };
    json!({
        "success": true,
        "ok": ok,
        "failures": fails,
        "message": message,
    })
}

pub async fn recycle_restore(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data: NamesBody = crate::server::parse_json(&body)?;
    if data.names.is_empty() {
        return Err(ApiError::bad("未选择任何条目"));
    }
    let state2 = state.clone();
    let names = data.names;
    let (ok, fails) = tokio::task::spawn_blocking(move || recycle::restore(&state2, &names))
        .await
        .map_err(|e| ApiError::internal(format!("还原失败：{e}")))?;
    Ok(Json(result_json(ok, fails, "还原")))
}

pub async fn recycle_purge(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data: NamesBody = crate::server::parse_json(&body)?;
    if data.names.is_empty() {
        return Err(ApiError::bad("未选择任何条目"));
    }
    let state2 = state.clone();
    let names = data.names;
    let (ok, fails) = tokio::task::spawn_blocking(move || recycle::purge(&state2, &names))
        .await
        .map_err(|e| ApiError::internal(format!("删除失败：{e}")))?;
    Ok(Json(result_json(ok, fails, "永久删除")))
}

pub async fn recycle_clear(state: State<SharedState>) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let state2 = state.clone();
    let (ok, fails) = tokio::task::spawn_blocking(move || recycle::clear(&state2))
        .await
        .map_err(|e| ApiError::internal(format!("清空回收站失败：{e}")))?;
    Ok(Json(result_json(ok, fails, "清空回收站")))
}

fn sanitize_backend(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-' | '_' | '[' | ']'))
        .collect();
    if cleaned.is_empty() {
        "127.0.0.1".into()
    } else {
        cleaned
    }
}

pub fn build_nginx_config(backend: &str, prefix: &str, listen_port: &str) -> String {
    let backend = sanitize_backend(backend);
    let prefix = auth::sanitize_prefix(prefix);
    let listen = if listen_port.trim().chars().all(|c| c.is_ascii_digit())
        && !listen_port.trim().is_empty()
    {
        listen_port.trim().to_string()
    } else {
        "9000".into()
    };
    let mut lines: Vec<String> = vec![
        "server {".into(),
        format!("    listen {listen};"),
        String::new(),
    ];
    if prefix.is_empty() {
        lines.push("    location / {".into());
    } else {
        lines.push(format!("    location = {prefix} {{"));
        lines.push(format!("        return 301 {prefix}/;"));
        lines.push("    }".into());
        lines.push(String::new());
        lines.push(format!("    location {prefix}/ {{"));
    }
    lines.push(format!("        proxy_pass http://{backend}/;"));
    if !prefix.is_empty() {
        lines.push(format!(
            "        proxy_set_header X-Forwarded-Prefix {prefix};"
        ));
    }
    lines.push("        proxy_set_header Host $host;".into());
    lines.push("        proxy_set_header X-Real-IP $remote_addr;".into());
    lines.push("        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;".into());
    lines.push("        proxy_http_version 1.1;".into());
    lines.push("        proxy_read_timeout 3600s;".into());
    lines.push("        proxy_send_timeout 3600s;".into());
    lines.push("        client_max_body_size 0;".into());
    lines.push("        proxy_request_buffering off;".into());
    lines.push("    }".into());
    lines.push(String::new());
    lines.push("    gzip on;".into());
    lines.push(
        "    gzip_types text/css application/javascript application/json image/svg+xml;".into(),
    );
    lines.push("    gzip_min_length 2048;".into());
    lines.push("}".into());
    lines.join("\n")
}

pub async fn nginx_config(Query(query): Query<HashMap<String, String>>) -> Response {
    let backend = query
        .get("backend")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".into());
    let prefix = query
        .get("prefix")
        .cloned()
        .unwrap_or_else(|| "file".into());
    let listen = query
        .get("listen")
        .cloned()
        .unwrap_or_else(|| "9000".into());
    let text = build_nginx_config(&backend, &prefix, &listen);
    let mut response = text.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nginx_template() {
        let text = build_nginx_config("192.168.1.2:8888", "/file/", "9000");
        assert!(text.contains("proxy_pass http://192.168.1.2:8888/;"));
        assert!(text.contains("X-Forwarded-Prefix /file;"));
        assert!(text.contains("location = /file {"));
    }
}
