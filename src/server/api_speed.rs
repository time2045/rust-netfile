use crate::error::{ApiError, ApiResult};
use axum::body::{Body, Bytes};
use axum::extract::{Query, Request};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

static ZERO_CHUNK: [u8; 65536] = [0u8; 65536];
const MAX_SPEED_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Deserialize, Default)]
pub struct SpeedQuery {
    bytes: Option<String>,
}

pub async fn down(Query(query): Query<SpeedQuery>) -> Response {
    let total = query
        .bytes
        .as_deref()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(8 * 1024 * 1024)
        .min(MAX_SPEED_BYTES);
    let stream = futures_util::stream::unfold(total, |remaining| async move {
        if remaining == 0 {
            return None;
        }
        let take = remaining.min(ZERO_CHUNK.len() as u64) as usize;
        let chunk = Bytes::from_static(&ZERO_CHUNK[..take]);
        Some((Ok::<Bytes, std::io::Error>(chunk), remaining - take as u64))
    });
    let mut response = Body::from_stream(stream).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from_str(&total.to_string()).unwrap(),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

pub async fn up(req: Request) -> ApiResult<Json<serde_json::Value>> {
    let content_length = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if content_length
        .map(|len| len > MAX_SPEED_BYTES)
        .unwrap_or(false)
    {
        return Err(ApiError::too_large("测速数据超出限制"));
    }
    let mut stream = req.into_body().into_data_stream();
    let mut count = 0u64;
    while let Some(item) = stream.next().await {
        let data = item.map_err(|e| ApiError::bad(format!("测速数据读取失败：{e}")))?;
        count += data.len() as u64;
        if count > MAX_SPEED_BYTES {
            return Err(ApiError::too_large("测速数据超出限制"));
        }
    }
    Ok(Json(json!({ "success": true, "bytes": count })))
}
