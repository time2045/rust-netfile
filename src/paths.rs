use crate::config::{RECYCLE_DIR, STAGING_DIR};
use crate::error::{ApiError, ApiResult};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DIR_CACHE_TTL: Duration = Duration::from_secs(60);
const DIR_CACHE_MAX: usize = 500;
const DIR_WALK_LIMIT: u64 = 100_000;

pub fn is_system_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case(RECYCLE_DIR) || name.eq_ignore_ascii_case(STAGING_DIR)
}

pub fn format_size(size: u64) -> String {
    let mut value = size as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if value < 1024.0 {
            return format!("{value:.1} {unit}");
        }
        value /= 1024.0;
    }
    format!("{value:.1} TB")
}

pub fn system_time_secs(time: SystemTime) -> f64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn format_time(time: SystemTime) -> String {
    let secs = system_time_secs(time) as i64;
    chrono::DateTime::<chrono::Local>::from(UNIX_EPOCH + Duration::from_secs(secs.max(0) as u64))
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

pub fn format_time_text(secs: f64) -> String {
    let secs = secs.max(0.0) as i64;
    chrono::DateTime::<chrono::Local>::from(UNIX_EPOCH + Duration::from_secs(secs as u64))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn contains_nul(text: &str) -> bool {
    text.contains('\0')
}

fn rel_components(rel: &str) -> Result<Vec<String>, ApiError> {
    if contains_nul(rel) {
        return Err(ApiError::bad("路径包含非法字符"));
    }
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_matches('/');
    if rel.is_empty() {
        return Ok(Vec::new());
    }
    let mut parts = Vec::new();
    for part in rel.split('/') {
        if part.is_empty() {
            return Err(ApiError::bad("路径格式不正确"));
        }
        if part == "." || part == ".." {
            return Err(ApiError::forbidden("路径不合法"));
        }
        if part.contains(':') {
            return Err(ApiError::forbidden("路径不合法"));
        }
        parts.push(part.to_string());
    }
    if parts.first().map(|p| is_system_dir(p)).unwrap_or(false) {
        return Err(ApiError::not_found("目标不存在"));
    }
    Ok(parts)
}

pub fn resolve_existing(root: &Path, rel: &str) -> ApiResult<PathBuf> {
    let parts = rel_components(rel)?;
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
    }
    let canonical =
        std::fs::canonicalize(&path).map_err(|_| ApiError::not_found("路径不存在或无法访问"))?;
    if !canonical.starts_with(root) {
        return Err(ApiError::forbidden("无权访问该路径"));
    }
    Ok(canonical)
}

pub fn resolve_for_create(root: &Path, rel: &str) -> ApiResult<PathBuf> {
    let parts = rel_components(rel)?;
    let mut path = root.to_path_buf();
    for part in &parts {
        path.push(part);
    }
    if path.exists() {
        let canonical = std::fs::canonicalize(&path)
            .map_err(|_| ApiError::not_found("路径不存在或无法访问"))?;
        if !canonical.starts_with(root) {
            return Err(ApiError::forbidden("无权访问该路径"));
        }
        return Ok(canonical);
    }
    let mut ancestor = path.clone();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name().map(|s| s.to_os_string()) else {
            return Err(ApiError::bad("目标路径不合法"));
        };
        rest.push(name);
        let Some(parent) = ancestor.parent() else {
            return Err(ApiError::bad("目标路径不合法"));
        };
        ancestor = parent.to_path_buf();
        if rest.len() > 256 {
            return Err(ApiError::bad("目标路径层级过深"));
        }
    }
    let canonical_ancestor = std::fs::canonicalize(&ancestor)
        .map_err(|_| ApiError::not_found("目标目录不存在或无法访问"))?;
    if !canonical_ancestor.starts_with(root) {
        return Err(ApiError::forbidden("无权访问该路径"));
    }
    let mut result = canonical_ancestor;
    for name in rest.into_iter().rev() {
        result.push(name);
    }
    Ok(result)
}

pub fn join_rel(current: &str, rel: &str) -> String {
    let current = current.trim_matches('/');
    let rel = rel.trim_matches('/');
    match (current.is_empty(), rel.is_empty()) {
        (true, true) => String::new(),
        (true, false) => rel.to_string(),
        (false, true) => current.to_string(),
        (false, false) => format!("{current}/{rel}"),
    }
}

const INVALID_NAME_CHARS: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("名称不能为空".into());
    }
    if name != name.trim() {
        return Err("名称不能以空格开头或结尾".into());
    }
    if name.len() > 200 {
        return Err("名称过长（最多 200 字节）".into());
    }
    if name.chars().any(|c| c.is_control()) || name.contains('\0') {
        return Err("名称不能包含控制字符".into());
    }
    if name == "." || name == ".." {
        return Err("名称不能为 . 或 ..".into());
    }
    if is_system_dir(name) {
        return Err(format!("名称不能使用系统保留名 {name}"));
    }
    if name.ends_with('.') {
        return Err("名称不能以点结尾".into());
    }
    if name.chars().any(|c| INVALID_NAME_CHARS.contains(&c)) {
        return Err("名称不能包含以下字符：\\ / : * ? \" < > |".into());
    }
    let upper = name.to_ascii_uppercase();
    let stem = upper.split('.').next().unwrap_or("");
    let reserved = matches!(
        stem,
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if reserved {
        return Err("名称不能使用 Windows 保留设备名（CON/PRN/AUX/NUL/COM*/LPT*）".into());
    }
    Ok(())
}

pub fn validate_rel_path(rel: &str) -> Result<(), String> {
    if rel.trim().is_empty() {
        return Err("路径不能为空".into());
    }
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_matches('/');
    if rel.is_empty() {
        return Err("路径不能为空".into());
    }
    for seg in rel.split('/') {
        if seg.is_empty() {
            return Err("路径格式不正确".into());
        }
        if seg == "." || seg == ".." {
            return Err("路径不能包含 . 或 ..".into());
        }
        validate_name(seg)?;
    }
    Ok(())
}

pub fn rel_string(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(v) => Some(v.to_string_lossy().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

pub fn auto_rename(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("item"));
    let ext = path.extension().map(|s| s.to_os_string());
    for i in 1..10_000u32 {
        let mut name = stem.clone();
        name.push(format!("({i})"));
        if let Some(ext) = &ext {
            name.push(".");
            name.push(ext);
        }
        let candidate = path.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[cfg(windows)]
pub fn is_link_like(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_type().is_symlink() || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

#[cfg(not(windows))]
pub fn is_link_like(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

pub fn dir_stats(path: &Path) -> (u64, u64) {
    let mut count = 0u64;
    let mut size = 0u64;
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            e.file_name()
                .to_str()
                .map(|n| !is_system_dir(n))
                .unwrap_or(true)
        })
        .flatten()
    {
        if entry.depth() == 0 {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if is_link_like(&meta) {
            continue;
        }
        if meta.is_file() {
            count += 1;
            size = size.saturating_add(meta.len());
        }
        if count >= DIR_WALK_LIMIT {
            break;
        }
    }
    (count, size)
}

struct DirCacheEntry {
    stamp: Instant,
    mtime: Option<SystemTime>,
    count: u64,
    size: u64,
}

pub struct DirSizeCache {
    inner: Mutex<HashMap<PathBuf, DirCacheEntry>>,
}

impl DirSizeCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, path: &Path) -> (u64, u64) {
        let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
        let now = Instant::now();
        if let Ok(guard) = self.inner.lock() {
            if let Some(hit) = guard.get(path) {
                if now.duration_since(hit.stamp) < DIR_CACHE_TTL && hit.mtime == mtime {
                    return (hit.count, hit.size);
                }
            }
        }
        let (count, size) = dir_stats(path);
        if let Ok(mut guard) = self.inner.lock() {
            if guard.len() >= DIR_CACHE_MAX {
                guard.clear();
            }
            guard.insert(
                path.to_path_buf(),
                DirCacheEntry {
                    stamp: now,
                    mtime,
                    count,
                    size,
                },
            );
        }
        (count, size)
    }

    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.clear();
        }
    }
}

impl Default for DirSizeCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nb-paths-{}", crate::config::random_hex(8)));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        std::fs::canonicalize(dir).unwrap()
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("正常文件.txt").is_ok());
        assert!(validate_name("CON.txt").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name(".recycle").is_err());
        assert!(validate_name("trail.").is_err());
        assert!(validate_name(" lead").is_err());
    }

    #[test]
    fn traversal_rejected() {
        let root = test_root();
        assert!(resolve_existing(&root, "../x").is_err());
        assert!(resolve_existing(&root, "sub/../a.txt").is_err());
        assert!(resolve_existing(&root, "a.txt").is_ok());
        assert!(resolve_existing(&root, ".recycle/meta.json").is_err());
        let created = resolve_for_create(&root, "new/deep/file.txt").unwrap();
        assert!(created.starts_with(&root));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn size_formatting() {
        assert_eq!(format_size(0), "0.0 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn auto_rename_generates_unique() {
        let root = test_root();
        std::fs::write(root.join("a(1).txt"), b"x").unwrap();
        let candidate = auto_rename(&root.join("a.txt"));
        assert_eq!(candidate.file_name().unwrap(), "a(2).txt");
        std::fs::remove_dir_all(root).ok();
    }
}
