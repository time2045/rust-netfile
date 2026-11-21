use crate::auth::client_ip;
use crate::config::{upload_limit_bytes, ConflictMode};
use crate::error::{ApiError, ApiResult};
use crate::paths::{format_size, join_rel, resolve_for_create, validate_rel_path};
use crate::recycle;
use crate::server::{get_str, json_value, shared};
use crate::state::SharedState;
use crate::uploads::{
    load_meta, meta_matches, meta_path, part_path, save_meta, valid_id, UploadMeta,
};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Query, Request, State};
use axum::http::{header, HeaderMap};
use axum::Json;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::Path;
use tokio::io::AsyncWriteExt;

const MAX_CHUNK_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Deserialize, Default)]
pub struct UploadQuery {
    upload_id: Option<String>,
    offset: Option<String>,
    total_size: Option<String>,
    rel_path: Option<String>,
    current_path: Option<String>,
}

fn parse_u64(text: Option<&String>, label: &str) -> Result<u64, ApiError> {
    match text {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| ApiError::bad(format!("{label}格式错误"))),
        None => Ok(0),
    }
}

fn validate_upload(rel: &str) -> Result<(), ApiError> {
    validate_rel_path(rel).map_err(|e| ApiError::bad(format!("上传被拒绝：{e}")))
}

async fn remove_staging(root: &Path, id: &str) {
    let _ = tokio::fs::remove_file(part_path(root, id)).await;
    let _ = tokio::fs::remove_file(meta_path(root, id)).await;
}

pub async fn status(
    state: State<SharedState>,
    Query(query): Query<UploadQuery>,
) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let id = query.upload_id.unwrap_or_default();
    if !valid_id(&id) {
        return Err(ApiError::bad("无效的上传标识"));
    }
    let rel = query.rel_path.unwrap_or_default();
    validate_upload(&rel)?;
    let current_path = query.current_path.unwrap_or_default();
    let total = parse_u64(query.total_size.as_ref(), "总大小")?;
    let root = state.root();
    let lock = state.uploads.lock_for(&id);
    let _guard = lock.lock().await;
    let part = part_path(&root, &id);
    let meta_file = meta_path(&root, &id);
    let mut offset = 0u64;
    let mut complete = false;
    if let Ok(meta) = tokio::fs::metadata(&part).await {
        if meta.len() > 0 {
            let meta_json = load_meta(&meta_file);
            let matches = meta_json
                .as_ref()
                .map(|m| meta_matches(m, &rel, &current_path, total))
                .unwrap_or(false);
            if matches {
                offset = meta.len();
                if total > 0 && offset > total {
                    remove_staging(&root, &id).await;
                    offset = 0;
                } else {
                    complete = total > 0 && offset == total;
                }
            } else {
                remove_staging(&root, &id).await;
            }
        }
    }
    Ok(Json(json!({
        "success": true,
        "offset": offset,
        "complete": complete,
        "total_size": total,
    })))
}

pub async fn chunk(
    state: State<SharedState>,
    Query(query): Query<UploadQuery>,
    req: Request,
) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let id = query.upload_id.unwrap_or_default();
    if !valid_id(&id) {
        return Err(ApiError::bad("无效的上传标识"));
    }
    let rel = query.rel_path.unwrap_or_default();
    validate_upload(&rel)?;
    let current_path = query.current_path.unwrap_or_default();
    let total = parse_u64(query.total_size.as_ref(), "总大小")?;
    let offset = parse_u64(query.offset.as_ref(), "偏移")?;
    let cfg = state.cfg();
    let max_size = upload_limit_bytes(&cfg);
    if max_size > 0 && total > max_size {
        return Err(ApiError::too_large(format!(
            "文件超出大小限制（{}）",
            format_size(max_size)
        )));
    }
    let content_length = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if content_length
        .map(|len| len > MAX_CHUNK_BYTES)
        .unwrap_or(false)
    {
        return Err(ApiError::too_large("单个上传分片过大"));
    }
    let root = state.root();
    let lock = state.uploads.lock_for(&id);
    let _guard = lock.lock().await;
    let part = part_path(&root, &id);
    let meta_file = meta_path(&root, &id);
    if let Some(parent) = part.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::internal(format!("无法创建临时目录：{e}")))?;
    }
    let meta = load_meta(&meta_file);
    let matches = meta
        .as_ref()
        .map(|m| meta_matches(m, &rel, &current_path, total))
        .unwrap_or(false);
    let mut current = 0u64;
    if !matches {
        remove_staging(&root, &id).await;
        let new_meta = UploadMeta {
            rel_path: rel.clone(),
            current_path: current_path.clone(),
            total_size: total,
            created: chrono::Local::now().timestamp() as f64,
        };
        save_meta(&meta_file, &new_meta)
            .map_err(|e| ApiError::internal(format!("无法写入上传元数据：{e}")))?;
    } else if let Ok(meta) = tokio::fs::metadata(&part).await {
        current = meta.len();
    }
    if offset != current {
        return Err(
            ApiError::conflict("偏移不一致，请重新同步").with_extra(json!({ "offset": current }))
        );
    }
    if total > 0 && current > total {
        remove_staging(&root, &id).await;
        return Err(ApiError::too_large("数据超出声明大小"));
    }
    if total > 0 && current == total {
        return Ok(Json(json!({ "success": true, "offset": current })));
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part)
        .await
        .map_err(|e| ApiError::internal(format!("无法打开临时文件：{e}")))?;
    let mut body = req.into_body().into_data_stream();
    let mut written = 0u64;
    while let Some(item) = body.next().await {
        let data = item.map_err(|e| ApiError::bad(format!("上传数据读取中断：{e}")))?;
        written += data.len() as u64;
        if written > MAX_CHUNK_BYTES
            || (total == 0 && written > 0)
            || (total > 0 && current + written > total)
        {
            drop(file);
            remove_staging(&root, &id).await;
            return Err(ApiError::too_large("数据超出声明大小"));
        }
        file.write_all(&data)
            .await
            .map_err(|e| ApiError::internal(format!("写入临时文件失败：{e}")))?;
    }
    file.flush()
        .await
        .map_err(|e| ApiError::internal(format!("写入临时文件失败：{e}")))?;
    drop(file);
    let newsize = current + written;
    Ok(Json(json!({ "success": true, "offset": newsize })))
}

fn mode_from(text: Option<&str>, default: ConflictMode) -> Result<ConflictMode, ApiError> {
    match text.map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("") => Ok(default),
        Some("ask") => Ok(ConflictMode::Ask),
        Some("rename") => Ok(ConflictMode::Rename),
        Some("overwrite") => Ok(ConflictMode::Overwrite),
        Some(other) => Err(ApiError::bad(format!("未知的冲突处理方式：{other}"))),
    }
}

async fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last_err = None;
    for attempt in 0..5 {
        match tokio::fs::rename(from, to).await {
            Ok(_) => return Ok(()),
            Err(err) => {
                last_err = Some(err);
                if attempt < 4 {
                    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "重命名失败")))
}

pub async fn complete(
    state: State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let id = get_str(&data, "upload_id")?;
    if !valid_id(&id) {
        return Err(ApiError::bad("无效的上传标识"));
    }
    let rel = data
        .get("rel_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    validate_upload(&rel)?;
    let current_path = data
        .get("current_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let total = data
        .get("total_size")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ApiError::bad("缺少必要参数：total_size"))?;
    let cfg = state.cfg();
    let max_size = upload_limit_bytes(&cfg);
    if max_size > 0 && total > max_size {
        return Err(ApiError::too_large(format!(
            "文件超出大小限制（{}）",
            format_size(max_size)
        )));
    }
    let mode = mode_from(data.get("mode").and_then(|v| v.as_str()), cfg.conflict_mode)?;
    let ip = client_ip(&headers, &addr);
    let root = state.root();
    let lock = state.uploads.lock_for(&id);
    let _guard = lock.lock().await;
    let part = part_path(&root, &id);
    let meta_file = meta_path(&root, &id);
    let part_meta = tokio::fs::metadata(&part).await.ok();
    let part_size = part_meta.as_ref().map(|m| m.len()).unwrap_or(0);
    if part_meta.is_some() {
        let meta_ok = load_meta(&meta_file)
            .as_ref()
            .map(|m| meta_matches(m, &rel, &current_path, total))
            .unwrap_or(false);
        if !meta_ok {
            remove_staging(&root, &id).await;
            return Err(ApiError::conflict("上传元数据不匹配，请重新上传"));
        }
    }
    if total > 0 && part_size != total {
        return Err(
            ApiError::conflict("文件尚未上传完整").with_extra(json!({ "offset": part_size }))
        );
    }
    if total == 0 && part_size > 0 {
        remove_staging(&root, &id).await;
        return Err(ApiError::conflict("文件尚未上传完整"));
    }
    let target = resolve_for_create(&root, &join_rel(&current_path, &rel))?;
    if target == root {
        return Err(ApiError::bad("不允许上传到共享根目录"));
    }
    if mode == ConflictMode::Ask && (target.exists() || state.uploads.is_reserved(&target)) {
        return Err(
            ApiError::conflict(format!("目标位置已存在同名项目（{rel}）"))
                .with_extra(json!({ "conflict": true })),
        );
    }
    if mode == ConflictMode::Overwrite && target.is_dir() {
        return Err(
            ApiError::conflict(format!("目标位置已存在同名文件夹（{rel}），无法覆盖"))
                .with_extra(json!({ "conflict": true })),
        );
    }
    let Some(reservation) = state.uploads.reserve(&target, mode) else {
        return Err(ApiError::conflict("目标文件正在被其他上传占用，请稍后重试")
            .with_extra(json!({ "conflict": true })));
    };
    let target = reservation.path().to_path_buf();
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::internal(format!("无法创建目标文件夹：{e}")))?;
    }
    let mut backup: Option<std::path::PathBuf> = None;
    if target.exists() && target.is_file() {
        backup = recycle::move_to_recycle(&state, &target).ok();
    }
    let result = if part_meta.is_some() {
        rename_with_retry(&part, &target).await
    } else {
        tokio::fs::File::create(&target).await.map(|_| ())
    };
    if let Err(err) = result {
        if let Some(backup) = backup {
            let _ = tokio::fs::rename(&backup, &target).await;
        }
        return Err(ApiError::internal(format!("保存文件失败：{err}")));
    }
    let _ = tokio::fs::remove_file(&meta_file).await;
    let display = crate::paths::rel_string(&root, &target);
    let filename = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or(display.clone());
    state.log.audit("upload", &display, total, &ip, "成功");
    state.stats.add_upload(total);
    state.dir_sizes.invalidate();
    state.log.line(&format!(
        "上传完成：{filename}（{}）到 /{current_path}",
        format_size(total)
    ));
    Ok(Json(json!({ "success": true, "path": display })))
}

pub async fn abort(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let id = get_str(&data, "upload_id")?;
    if !valid_id(&id) {
        return Err(ApiError::bad("无效的上传标识"));
    }
    let root = state.root();
    let lock = state.uploads.lock_for(&id);
    let _guard = lock.lock().await;
    remove_staging(&root, &id).await;
    Ok(Json(json!({ "success": true })))
}
