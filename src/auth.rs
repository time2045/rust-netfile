use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use axum::http::{header, HeaderMap, Method};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

pub const SESSION_COOKIE: &str = "nb_session";
pub const CSRF_COOKIE: &str = "nb_csrf";
pub const CSRF_HEADER: &str = "x-csrf-token";

pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("密码哈希失败：{e}"))
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn validate_password(password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("密码不能为空".into());
    }
    if password.len() > 128 {
        return Err("密码长度不能超过 128 字节".into());
    }
    if password.chars().any(|c| c.is_control()) {
        return Err("密码不能包含控制字符".into());
    }
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn session_signature(secret: &str, epoch: &str, expires: u64) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key");
    mac.update(format!("nb-session:v1:{epoch}:{expires}").as_bytes());
    hex(&mac.finalize().into_bytes())
}

pub fn create_session_value(secret: &str, epoch: &str, hours: u32) -> String {
    let expires = now_secs() + (hours as u64) * 3600;
    format!("{expires}.{}", session_signature(secret, epoch, expires))
}

pub fn verify_session_value(secret: &str, epoch: &str, value: &str) -> Option<u64> {
    let (exp_text, sig) = value.split_once('.')?;
    let expires: u64 = exp_text.parse().ok()?;
    if expires <= now_secs() {
        return None;
    }
    let expected = session_signature(secret, epoch, expires);
    if expected.as_bytes().ct_eq(sig.as_bytes()).unwrap_u8() == 1 {
        Some(expires)
    } else {
        None
    }
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    for value in headers.get_all(header::COOKIE).iter() {
        let Ok(text) = value.to_str() else { continue };
        for part in text.split(';') {
            let part = part.trim();
            if let Some((k, v)) = part.split_once('=') {
                if k.trim() == name {
                    return Some(v.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    None
}

fn cookie_string(
    name: &str,
    value: &str,
    max_age: Option<i64>,
    http_only: bool,
    secure: bool,
) -> String {
    let mut cookie = format!("{name}={value}; Path=/; SameSite=Lax");
    if http_only {
        cookie.push_str("; HttpOnly");
    }
    if secure {
        cookie.push_str("; Secure");
    }
    if let Some(age) = max_age {
        cookie.push_str(&format!("; Max-Age={age}"));
    }
    cookie
}

pub fn session_cookie(value: &str, max_age_secs: u32, secure: bool) -> String {
    cookie_string(
        SESSION_COOKIE,
        value,
        Some(max_age_secs as i64),
        true,
        secure,
    )
}

pub fn clear_session_cookie(secure: bool) -> String {
    cookie_string(SESSION_COOKIE, "", Some(0), true, secure)
}

pub fn csrf_cookie(value: &str) -> String {
    cookie_string(CSRF_COOKIE, value, Some(30 * 24 * 3600), false, false)
}

pub fn csrf_matches(headers: &HeaderMap) -> bool {
    let Some(cookie) = cookie_value(headers, CSRF_COOKIE) else {
        return false;
    };
    let Some(header_token) = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    !cookie.is_empty() && cookie.as_bytes().ct_eq(header_token.as_bytes()).unwrap_u8() == 1
}

pub fn is_mutating(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

pub fn trusted_proxy(remote: &SocketAddr) -> bool {
    remote.ip().is_loopback()
}

pub fn client_ip(headers: &HeaderMap, remote: &SocketAddr) -> String {
    if trusted_proxy(remote) {
        if let Some(real) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
            if let Ok(ip) = real.trim().parse::<IpAddr>() {
                return ip.to_string();
            }
        }
        if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            if let Some(first) = xff.split(',').next() {
                if let Ok(ip) = first.trim().parse::<IpAddr>() {
                    return ip.to_string();
                }
            }
        }
    }
    remote.ip().to_string()
}

pub fn sanitize_prefix(raw: &str) -> String {
    let mut segments = Vec::new();
    for seg in raw.split('/') {
        let seg = seg.trim();
        if seg.is_empty() || seg == "." || seg == ".." {
            continue;
        }
        if !seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            continue;
        }
        segments.push(seg);
    }
    if segments.is_empty() {
        String::new()
    } else {
        format!("/{}", segments.join("/"))
    }
}

pub fn forwarded_prefix(headers: &HeaderMap, remote: &SocketAddr) -> String {
    if !trusted_proxy(remote) {
        return String::new();
    }
    headers
        .get("x-forwarded-prefix")
        .and_then(|v| v.to_str().ok())
        .map(sanitize_prefix)
        .unwrap_or_default()
}

pub fn request_is_secure(headers: &HeaderMap, remote: &SocketAddr) -> bool {
    trusted_proxy(remote)
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("https"))
            .unwrap_or(false)
}

struct Attempt {
    failures: u32,
    window_start: Instant,
    blocked_until: Option<Instant>,
}

pub struct LoginLimiter {
    inner: Mutex<HashMap<String, Attempt>>,
}

impl LoginLimiter {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn retry_after(&self, ip: &str) -> Option<u64> {
        let mut guard = self.lock();
        let now = Instant::now();
        if let Some(attempt) = guard.get(ip) {
            if let Some(blocked) = attempt.blocked_until {
                if blocked > now {
                    return Some(blocked.duration_since(now).as_secs().max(1));
                }
            }
        }
        guard.retain(|_, a| {
            a.blocked_until.map(|t| t > now).unwrap_or(false)
                || now.duration_since(a.window_start) < Duration::from_secs(3600)
        });
        None
    }

    pub fn record_failure(&self, ip: &str, max_failures: u32, block_minutes: u32) -> Option<u64> {
        let mut guard = self.lock();
        let now = Instant::now();
        let attempt = guard.entry(ip.to_string()).or_insert(Attempt {
            failures: 0,
            window_start: now,
            blocked_until: None,
        });
        if attempt.blocked_until.map(|t| t > now).unwrap_or(false) {
            let until = attempt.blocked_until.unwrap();
            return Some(until.duration_since(now).as_secs().max(1));
        }
        if now.duration_since(attempt.window_start) > Duration::from_secs(600) {
            attempt.failures = 0;
            attempt.window_start = now;
        }
        attempt.failures += 1;
        if attempt.failures >= max_failures.max(1) {
            let until = now + Duration::from_secs(block_minutes.max(1) as u64 * 60);
            attempt.blocked_until = Some(until);
            attempt.failures = 0;
            return Some(block_minutes.max(1) as u64 * 60);
        }
        None
    }

    pub fn reset(&self, ip: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(ip);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Attempt>> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }
}

impl Default for LoginLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_roundtrip_and_tamper() {
        let secret = crate::config::random_hex(32);
        let epoch = crate::config::random_hex(16);
        let value = create_session_value(&secret, &epoch, 8);
        assert!(verify_session_value(&secret, &epoch, &value).is_some());
        assert!(verify_session_value(&secret, &crate::config::random_hex(16), &value).is_none());
        let mut tampered = value.clone();
        tampered.push('a');
        assert!(verify_session_value(&secret, &epoch, &tampered).is_none());
    }

    #[test]
    fn limiter_blocks_after_failures() {
        let limiter = LoginLimiter::new();
        for _ in 0..4 {
            assert!(limiter.record_failure("1.2.3.4", 5, 10).is_none());
        }
        assert!(limiter.record_failure("1.2.3.4", 5, 10).is_some());
        assert!(limiter.retry_after("1.2.3.4").is_some());
        limiter.reset("1.2.3.4");
        assert!(limiter.retry_after("1.2.3.4").is_none());
    }

    #[test]
    fn prefix_sanitizing() {
        assert_eq!(sanitize_prefix("/file/"), "/file");
        assert_eq!(sanitize_prefix("a//b/../c"), "/a/b/c");
        assert_eq!(sanitize_prefix("中文"), "");
        assert_eq!(sanitize_prefix(""), "");
    }

    #[test]
    fn password_hashing() {
        let hash = hash_password("测试密码123").unwrap();
        assert!(verify_password(&hash, "测试密码123"));
        assert!(!verify_password(&hash, "wrong"));
    }
}
