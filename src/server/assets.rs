use crate::state::AppState;
use axum::http::{header, HeaderName, HeaderValue};
use axum::response::{IntoResponse, Response};

pub const INDEX_HTML: &str = include_str!("../../web/index.html");
pub const LOGIN_HTML: &str = include_str!("../../web/login.html");
pub const FAVICON: &[u8] = include_bytes!("../../icons/favicon.ico");

pub fn html_response(html: &'static str, _state: &AppState) -> Response {
    let mut response = html.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
        ),
    );
    response
}

pub fn static_asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    Some(match path {
        "css/base.css" => (
            "text/css; charset=utf-8",
            include_bytes!("../../web/css/base.css").as_slice(),
        ),
        "css/files.css" => (
            "text/css; charset=utf-8",
            include_bytes!("../../web/css/files.css").as_slice(),
        ),
        "css/login.css" => (
            "text/css; charset=utf-8",
            include_bytes!("../../web/css/login.css").as_slice(),
        ),
        "js/core.js" => (
            "application/javascript; charset=utf-8",
            include_bytes!("../../web/js/core.js").as_slice(),
        ),
        "js/files.js" => (
            "application/javascript; charset=utf-8",
            include_bytes!("../../web/js/files.js").as_slice(),
        ),
        "js/app.js" => (
            "application/javascript; charset=utf-8",
            include_bytes!("../../web/js/app.js").as_slice(),
        ),
        "js/login.js" => (
            "application/javascript; charset=utf-8",
            include_bytes!("../../web/js/login.js").as_slice(),
        ),
        _ => return None,
    })
}
