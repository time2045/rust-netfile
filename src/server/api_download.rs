use crate::archive::{build_zip, unique_top_names, ZipSpec};
use crate::auth::client_ip;
use crate::error::{ApiError, ApiResult};
use crate::paths::resolve_existing;
use crate::server::{get_str, get_str_opt, json_value, shared};
use crate::state::SharedState;
use crate::tasks::TaskResult;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::Stream;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

#[derive(Deserialize, Default)]
pub struct DownloadQuery {
    path: Option<String>,
    inline: Option<String>,
    task_id: Option<String>,
}

pub fn parse_range(header_value: Option<&str>, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(raw) = header_value else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.contains(',') {
        return Ok(None);
    }
    let Some(spec) = raw.strip_prefix("bytes=") else {
        return Err(());
    };
    let Some((start_text, end_text)) = spec.split_once('-') else {
        return Err(());
    };
    if size == 0 {
        return Err(());
    }
    if start_text.trim().is_empty() {
        let suffix: u64 = end_text.trim().parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let start = size.saturating_sub(suffix);
        return Ok(Some((start, size - 1)));
    }
    let start: u64 = start_text.trim().parse().map_err(|_| ())?;
    if start >= size {
        return Err(());
    }
    let end = if end_text.trim().is_empty() {
        size - 1
    } else {
        let end: u64 = end_text.trim().parse().map_err(|_| ())?;
        end.min(size - 1)
    };
    if end < start {
        return Err(());
    }
    Ok(Some((start, end)))
}

fn clean_fallback(filename: &str) -> String {
    filename
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_control() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn content_disposition(filename: &str, inline: bool) -> String {
    let kind = if inline { "inline" } else { "attachment" };
    let fallback = clean_fallback(filename);
    let encoded = utf8_percent_encode(filename, NON_ALPHANUMERIC).to_string();
    format!("{kind}; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

fn inline_safe(mime: &str) -> bool {
    if mime == "image/svg+xml" || mime == "text/html" || mime == "application/xhtml+xml" {
        return false;
    }
    mime.starts_with("image/") || mime.starts_with("video/") || mime.starts_with("audio/")
}

fn mime_for(path: &std::path::Path) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}

fn range_not_satisfiable(size: u64) -> Response {
    let mut response = (StatusCode::RANGE_NOT_SATISFIABLE, "请求范围无效").into_response();
    response.headers_mut().insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{size}")).unwrap(),
    );
    response
}

#[derive(Clone)]
struct ZipCleanup {
    state: SharedState,
    task_id: String,
    path: PathBuf,
}

impl ZipCleanup {
    fn run(self) {
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
            self.state.tasks.remove(&self.task_id);
            let _ = tokio::fs::remove_file(&self.path).await;
        });
    }
}

type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

fn reader_body(reader: tokio::fs::File, length: u64) -> Body {
    let stream = ReaderStream::new(reader.take(length));
    Body::from_stream(stream)
}

fn zip_file_body(file: tokio::fs::File, length: u64, cleanup: Option<ZipCleanup>) -> Body {
    if cleanup.is_none() {
        return reader_body(file, length);
    }
    let stream: ByteStream = Box::pin(futures_util::stream::unfold(
        Some((file, length, cleanup)),
        |state| async move {
            let (mut file, remaining, cleanup) = state?;
            if remaining == 0 {
                if let Some(cleanup) = cleanup {
                    cleanup.run();
                }
                return None;
            }
            let mut buffer = vec![0u8; 256 * 1024];
            match file.read(&mut buffer).await {
                Ok(0) => {
                    if let Some(cleanup) = cleanup {
                        cleanup.run();
                    }
                    None
                }
                Ok(n) => {
                    let n = (n as u64).min(remaining) as usize;
                    let chunk = Bytes::copy_from_slice(&buffer[..n]);
                    let left = remaining - n as u64;
                    let mut cleanup = cleanup;
                    if left == 0 {
                        if let Some(cleanup) = cleanup.take() {
                            cleanup.run();
                        }
                    }
                    Some((Ok(chunk), Some((file, left, cleanup))))
                }
                Err(err) => Some((Err(err), None)),
            }
        },
    ));
    Body::from_stream(stream)
}

async fn stream_response(
    path: &std::path::Path,
    filename: &str,
    size: u64,
    range: Option<(u64, u64)>,
    mime: &str,
    inline: bool,
    cleanup: Option<ZipCleanup>,
) -> ApiResult<Response> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| ApiError::not_found(format!("无法打开文件：{e}")))?;
    let (start, end, length, partial) = match range {
        Some((start, end)) => (start, end, end - start + 1, true),
        None => (0, size.saturating_sub(1), size, false),
    };
    if let Err(err) = file.seek(std::io::SeekFrom::Start(start)).await {
        return Err(ApiError::internal(format!("读取文件失败：{err}")));
    }
    let body = if size == 0 {
        Body::empty()
    } else if cleanup.is_some() {
        zip_file_body(file, length, cleanup)
    } else {
        reader_body(file, length)
    };
    let mut builder = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, mime)
        .header(
            header::CONTENT_DISPOSITION,
            content_disposition(filename, inline),
        )
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length.to_string());
    if partial {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(modified) = meta.modified() {
            builder = builder.header(header::LAST_MODIFIED, httpdate::fmt_http_date(modified));
        }
        if let Ok(modified) = meta.modified() {
            let stamp = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            builder = builder.header(header::ETAG, format!("\"{:x}-{:x}\"", size, stamp));
        }
    }
    builder
        .body(body)
        .map_err(|e| ApiError::internal(format!("构造响应失败：{e}")))
}

pub async fn download(
    state: State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<DownloadQuery>,
) -> ApiResult<Response> {
    let state = shared(state);
    let rel = query
        .path
        .unwrap_or_default()
        .replace('\\', "/")
        .trim_matches('/')
        .to_string();
    let root = state.root();
    let path = resolve_existing(&root, &rel)?;
    if path.is_dir() {
        state.log.audit(
            "download",
            if rel.is_empty() {
                "共享根目录"
            } else {
                &rel
            },
            0,
            &client_ip(&headers, &addr),
            "失败：目标为文件夹",
        );
        return Err(ApiError::bad("目标为文件夹，请使用“打包下载”功能"));
    }
    let meta = std::fs::metadata(&path).map_err(|_| ApiError::not_found("文件不存在"))?;
    if !meta.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    let size = meta.len();
    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let range = match parse_range(range_header.as_deref(), size) {
        Ok(range) => range,
        Err(_) => return Ok(range_not_satisfiable(size)),
    };
    let length = range.map(|(s, e)| e - s + 1).unwrap_or(size);
    let ip = client_ip(&headers, &addr);
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".into());
    state
        .log
        .audit("download", &filename, size, &ip, "开始下载");
    state.stats.add_download(length);
    let inline = matches!(query.inline.as_deref(), Some("1") | Some("true"));
    let mime = mime_for(&path);
    let allow_inline = inline && inline_safe(&mime);
    stream_response(&path, &filename, size, range, &mime, allow_inline, None).await
}

pub async fn preview(
    state: State<SharedState>,
    Query(query): Query<DownloadQuery>,
) -> ApiResult<Response> {
    let state = shared(state);
    let rel = query
        .path
        .unwrap_or_default()
        .replace('\\', "/")
        .trim_matches('/')
        .to_string();
    let root = state.root();
    let path = resolve_existing(&root, &rel)?;
    let meta = std::fs::metadata(&path).map_err(|_| ApiError::not_found("文件不存在"))?;
    if !meta.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let text_like = matches!(
        ext.as_str(),
        "txt"
            | "md"
            | "log"
            | "json"
            | "xml"
            | "csv"
            | "yml"
            | "yaml"
            | "toml"
            | "ini"
            | "conf"
            | "cfg"
            | "rs"
            | "go"
            | "py"
            | "js"
            | "ts"
            | "css"
            | "html"
            | "htm"
            | "sh"
            | "bat"
            | "ps1"
            | "sql"
            | "c"
            | "cpp"
            | "h"
            | "java"
            | "php"
            | "vue"
    );
    if !text_like {
        return Err(ApiError::bad("该文件类型不支持文本预览"));
    }
    const LIMIT: u64 = 512 * 1024;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| ApiError::internal(format!("读取文件失败：{e}")))?;
    let mut buffer = Vec::new();
    file.take(LIMIT)
        .read_to_end(&mut buffer)
        .await
        .map_err(|e| ApiError::internal(format!("读取文件失败：{e}")))?;
    let truncated = meta.len() > LIMIT;
    let text = String::from_utf8_lossy(&buffer).to_string();
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut response = text.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(&filename, true))
            .unwrap_or_else(|_| HeaderValue::from_static("inline")),
    );
    if truncated {
        headers.insert(
            header::HeaderName::from_static("x-preview-truncated"),
            HeaderValue::from_static("1"),
        );
    }
    Ok(response)
}

fn zip_filename(folder: &str) -> String {
    format!("{folder}.zip")
}

async fn spawn_zip_task(
    state: SharedState,
    label: String,
    filename: String,
    specs: Vec<ZipSpec>,
    ip: String,
) -> String {
    let task = state.tasks.create("zip", label);
    let task_id = task.id.clone();
    let task_t = task.clone();
    let state_t = state.clone();
    let filename_t = filename.clone();
    tokio::task::spawn_blocking(move || {
        let zip_path = state_t.temp_dir().join(format!("{}.zip", task_t.id));
        let compress = state_t.cfg().zip_compress;
        match build_zip(&specs, &zip_path, compress, &task_t) {
            Ok(size) => {
                state_t.log.audit(
                    "download",
                    &format!("{filename_t}（打包）"),
                    size,
                    &ip,
                    "打包完成",
                );
                task_t.finish_done(
                    "打包完成",
                    Some(TaskResult {
                        path: zip_path,
                        filename: filename_t,
                        size,
                    }),
                );
            }
            Err(err) => {
                let _ = std::fs::remove_file(&zip_path);
                task_t.finish_failed(format!("打包失败：{err}"));
            }
        }
    });
    task_id
}

pub async fn download_folder(
    state: State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let ip = client_ip(&headers, &addr);
    let data = json_value(&body)?;
    let rel = get_str_opt(&data, "path").unwrap_or_default();
    let root = state.root();
    let folder = resolve_existing(&root, &rel)?;
    if !folder.is_dir() {
        return Err(ApiError::not_found("文件夹不存在"));
    }
    let name = folder
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "打包下载".into());
    let include_top = state.cfg().zip_include_folder;
    let specs = vec![ZipSpec {
        path: folder.clone(),
        prefix: if include_top {
            name.clone()
        } else {
            String::new()
        },
    }];
    state.log.line(&format!("打包下载文件夹：{}", name));
    let task_id = spawn_zip_task(
        state.clone(),
        format!("正在打包文件夹 {name} ..."),
        zip_filename(&name),
        specs,
        ip,
    )
    .await;
    Ok(Json(json!({ "success": true, "task_id": task_id })))
}

pub async fn download_many(
    state: State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let ip = client_ip(&headers, &addr);
    let data = json_value(&body)?;
    let paths: Vec<String> = data
        .get("paths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if paths.is_empty() {
        return Err(ApiError::bad("未选择任何项目"));
    }
    let root = state.root();
    let mut sources = Vec::new();
    for rel in paths {
        let path = resolve_existing(&root, &rel)?;
        if path == root {
            return Err(ApiError::bad("不允许打包下载共享根目录"));
        }
        sources.push(path);
    }
    let named = unique_top_names(&sources);
    let specs: Vec<ZipSpec> = named
        .into_iter()
        .map(|(path, prefix)| ZipSpec { path, prefix })
        .collect();
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let filename = format!("批量下载_{stamp}.zip");
    state
        .log
        .line(&format!("批量打包下载 {} 个项目", specs.len()));
    let task_id = spawn_zip_task(
        state.clone(),
        format!("正在打包 {} 个项目 ...", specs.len()),
        filename,
        specs,
        ip,
    )
    .await;
    Ok(Json(json!({ "success": true, "task_id": task_id })))
}

pub async fn task_status(
    state: State<SharedState>,
    Query(query): Query<DownloadQuery>,
) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let task_id = query.task_id.unwrap_or_default();
    let Some(task) = state.tasks.get(&task_id) else {
        return Err(ApiError::not_found("任务不存在或已过期"));
    };
    let view = task.view();
    Ok(Json(json!({
        "success": true,
        "status": view.status,
        "kind": view.kind,
        "message": view.message,
        "done": view.done,
        "total": view.total,
    })))
}

pub async fn task_cleanup(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let task_id = get_str(&data, "task_id")?;
    if let Some(task) = state.tasks.remove(&task_id) {
        if let Some((path, _name, _size)) = task.result() {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(Json(json!({ "success": true })))
}

pub async fn download_file(
    state: State<SharedState>,
    headers: HeaderMap,
    Query(query): Query<DownloadQuery>,
) -> ApiResult<Response> {
    let state = shared(state);
    let task_id = query.task_id.unwrap_or_default();
    let Some(task) = state.tasks.get(&task_id) else {
        return Err(ApiError::not_found("任务不存在或已过期"));
    };
    if task.view().status != "done" {
        return Err(ApiError::bad("任务尚未完成"));
    }
    let Some((path, filename, size)) = task.result() else {
        return Err(ApiError::not_found("打包结果不存在"));
    };
    task.touch_finished();
    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let range = match parse_range(range_header.as_deref(), size) {
        Ok(range) => range,
        Err(_) => return Ok(range_not_satisfiable(size)),
    };
    let length = range.map(|(s, e)| e - s + 1).unwrap_or(size);
    state.stats.add_download(length);
    let cleanup = if range.is_none() {
        Some(ZipCleanup {
            state: state.clone(),
            task_id: task_id.clone(),
            path: path.clone(),
        })
    } else {
        None
    };
    stream_response(
        &path,
        &filename,
        size,
        range,
        "application/zip",
        false,
        cleanup,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range(None, 100).unwrap(), None);
        assert_eq!(parse_range(Some("bytes=0-9"), 100).unwrap(), Some((0, 9)));
        assert_eq!(parse_range(Some("bytes=90-"), 100).unwrap(), Some((90, 99)));
        assert_eq!(parse_range(Some("bytes=-10"), 100).unwrap(), Some((90, 99)));
        assert_eq!(
            parse_range(Some("bytes=0-200"), 100).unwrap(),
            Some((0, 99))
        );
        assert!(parse_range(Some("bytes=100-"), 100).is_err());
        assert!(parse_range(Some("bytes=5-2"), 100).is_err());
        assert!(parse_range(Some("bytes=-0"), 100).is_err());
        assert_eq!(parse_range(Some("bytes=0-1,4-5"), 100).unwrap(), None);
    }

    #[test]
    fn dispositions() {
        let value = content_disposition("报告.pdf", false);
        assert!(value.starts_with("attachment;"));
        assert!(value.contains("filename*=UTF-8''"));
        assert!(inline_safe("image/png"));
        assert!(!inline_safe("image/svg+xml"));
        assert!(!inline_safe("text/html"));
    }
}
