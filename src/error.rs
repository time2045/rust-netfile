use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    pub extra: Option<Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            extra: None,
        }
    }

    pub fn with_extra(mut self, extra: Value) -> Self {
        self.extra = Some(extra);
        self
    }

    pub fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    pub fn too_large(message: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, message)
    }

    pub fn locked() -> Self {
        Self::new(
            StatusCode::LOCKED,
            "服务已锁定，请在桌面管理端或系统托盘解锁后重试",
        )
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = self.extra.unwrap_or_else(|| json!({}));
        if let Value::Object(map) = &mut body {
            map.insert("success".into(), Value::Bool(false));
            map.insert("message".into(), Value::String(self.message));
        } else {
            body = json!({ "success": false, "message": self.message });
        }
        (self.status, Json(body)).into_response()
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        ApiError::internal(format!("文件操作失败：{err}"))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        ApiError::bad(format!("请求数据格式错误：{err}"))
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

pub fn success_json() -> Json<Value> {
    Json(json!({ "success": true }))
}

pub fn json_success(message: Option<&str>) -> Json<Value> {
    match message {
        Some(m) => Json(json!({ "success": true, "message": m })),
        None => Json(json!({ "success": true })),
    }
}
