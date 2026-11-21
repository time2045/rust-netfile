use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "NB 文件管理器";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RECYCLE_DIR: &str = ".recycle";
pub const STAGING_DIR: &str = ".upload_tmp";
pub const CONFIG_FILE: &str = "config.json";
pub const TMP_DIR: &str = "tmp";
pub const LOGS_DIR: &str = "logs";

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    let mut out = String::with_capacity(bytes * 2);
    for b in buf {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn default_secret() -> String {
    random_hex(32)
}

fn default_auth_epoch() -> String {
    random_hex(16)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictMode {
    Ask,
    Rename,
    Overwrite,
}

impl ConflictMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictMode::Ask => "ask",
            ConflictMode::Rename => "rename",
            ConflictMode::Overwrite => "overwrite",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZipCompress {
    Standard,
    Fast,
    Store,
}

impl ZipCompress {
    pub fn method(&self) -> zip::CompressionMethod {
        match self {
            ZipCompress::Store => zip::CompressionMethod::Stored,
            _ => zip::CompressionMethod::Deflated,
        }
    }

    pub fn level(&self) -> Option<i64> {
        match self {
            ZipCompress::Standard => None,
            ZipCompress::Fast => Some(1),
            ZipCompress::Store => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub port: u16,
    pub bind_lan: bool,
    pub root_dir: String,
    pub password_hash: Option<String>,
    pub upload_limit_mb: u64,
    pub conflict_mode: ConflictMode,
    pub zip_compress: ZipCompress,
    pub zip_include_folder: bool,
    pub recycle_days: u32,
    pub session_hours: u32,
    pub max_login_failures: u32,
    pub login_block_minutes: u32,
    pub auto_start: bool,
    pub secret: String,
    pub auth_epoch: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: 8888,
            bind_lan: true,
            root_dir: String::new(),
            password_hash: None,
            upload_limit_mb: 0,
            conflict_mode: ConflictMode::Ask,
            zip_compress: ZipCompress::Standard,
            zip_include_folder: true,
            recycle_days: 30,
            session_hours: 8,
            max_login_failures: 5,
            login_block_minutes: 10,
            auto_start: true,
            secret: default_secret(),
            auth_epoch: default_auth_epoch(),
        }
    }
}

impl AppConfig {
    pub fn has_password(&self) -> bool {
        self.password_hash
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }

    pub fn is_valid_hex(value: &str, bytes: usize) -> bool {
        value.len() == bytes * 2 && value.chars().all(|c| c.is_ascii_hexdigit())
    }

    pub fn rotate_auth_epoch(&mut self) {
        self.auth_epoch = default_auth_epoch();
    }

    pub fn default_root(data_dir: &Path) -> PathBuf {
        data_dir.join("files")
    }
}

pub fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONFIG_FILE)
}

fn ensure_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("无法创建目录 {}：{e}", path.display()))
}

fn writable_dir(path: &Path) -> bool {
    if ensure_dir(path).is_err() {
        return false;
    }
    let probe = path.join(".netfile-write-test");
    match std::fs::write(&probe, b"ok") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn resolve_data_dir(exe_dir: Option<PathBuf>) -> PathBuf {
    if let Ok(custom) = std::env::var("NETFILE_DATA_DIR") {
        let mut p = PathBuf::from(custom);
        if p.is_relative() {
            if let Ok(cwd) = std::env::current_dir() {
                p = cwd.join(p);
            }
        }
        if writable_dir(&p) {
            return p;
        }
    }
    if let Some(base) = exe_dir {
        let candidate = base.join("data");
        if writable_dir(&candidate) {
            return candidate;
        }
    }
    let fallback = if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("NBNetFile"))
            .unwrap_or_else(|| std::env::temp_dir().join("NBNetFile"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|p| p.join("Library/Application Support/NBNetFile"))
            .unwrap_or_else(|| std::env::temp_dir().join("NBNetFile"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|p| p.join("nb-file-manager"))
            .unwrap_or_else(|| std::env::temp_dir().join("nb-file-manager"))
    };
    let _ = ensure_dir(&fallback);
    fallback
}

pub fn load(data_dir: &Path) -> (AppConfig, Option<String>) {
    let _ = ensure_dir(data_dir);
    let path = config_path(data_dir);
    let mut warning = None;
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<AppConfig>(&text) {
            Ok(cfg) => cfg,
            Err(err) => {
                let backup = data_dir.join(format!(
                    "config.json.bad-{}",
                    chrono::Local::now().format("%Y%m%d_%H%M%S")
                ));
                let _ = std::fs::rename(&path, &backup);
                warning = Some(format!(
                    "配置文件解析失败，已备份为 {}：{err}",
                    backup.display()
                ));
                AppConfig::default()
            }
        },
        Err(_) => AppConfig::default(),
    };

    if !AppConfig::is_valid_hex(&cfg.secret, 32) {
        cfg.secret = default_secret();
    }
    if !AppConfig::is_valid_hex(&cfg.auth_epoch, 16) {
        cfg.auth_epoch = default_auth_epoch();
    }
    if cfg.root_dir.trim().is_empty() {
        cfg.root_dir = AppConfig::default_root(data_dir)
            .to_string_lossy()
            .to_string();
    }
    if let Err(err) = normalize_root(&mut cfg, data_dir) {
        warning = Some(match warning {
            Some(prev) => format!("{prev}；{err}"),
            None => err,
        });
    }
    (cfg, warning)
}

pub fn normalize_root(cfg: &mut AppConfig, data_dir: &Path) -> Result<(), String> {
    let raw = cfg.root_dir.trim();
    if raw.is_empty() {
        return Err("共享目录不能为空".into());
    }
    let mut path = PathBuf::from(raw);
    if !path.is_absolute() {
        path = data_dir.join(path);
    }
    ensure_dir(&path)?;
    let canonical = std::fs::canonicalize(&path)
        .map_err(|e| format!("无法解析共享目录 {}：{e}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!("共享目录不是文件夹：{}", canonical.display()));
    }
    cfg.root_dir = clean_display_path(canonical).to_string_lossy().to_string();
    Ok(())
}

/// 去掉 Windows canonicalize 产生的 `\\?\` 前缀，便于配置与界面展示。
pub fn clean_display_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            if let Some(unc) = rest.strip_prefix("UNC\\") {
                return PathBuf::from(format!(r"\\{}", unc));
            }
            return PathBuf::from(rest);
        }
    }
    path
}

pub fn validate(cfg: &mut AppConfig, data_dir: &Path) -> Result<(), String> {
    if cfg.port == 0 {
        return Err("端口号必须在 1-65535 范围内".into());
    }
    normalize_root(cfg, data_dir)?;
    if cfg.upload_limit_mb > 10 * 1024 * 1024 {
        return Err("上传限制过大（最大 10 TB）".into());
    }
    if !(1..=3650).contains(&cfg.recycle_days) {
        return Err("回收站保留天数必须在 1-3650 之间".into());
    }
    if !(1..=720).contains(&cfg.session_hours) {
        return Err("登录有效期必须在 1-720 小时之间".into());
    }
    if !(3..=100).contains(&cfg.max_login_failures) {
        return Err("登录失败次数上限必须在 3-100 之间".into());
    }
    if !(1..=1440).contains(&cfg.login_block_minutes) {
        return Err("登录封禁时长必须在 1-1440 分钟之间".into());
    }
    if cfg.secret.is_empty() {
        cfg.secret = default_secret();
    }
    if cfg.auth_epoch.is_empty() {
        cfg.auth_epoch = default_auth_epoch();
    }
    Ok(())
}

pub fn save(data_dir: &Path, cfg: &AppConfig) -> Result<(), String> {
    ensure_dir(data_dir)?;
    let path = config_path(data_dir);
    let tmp = data_dir.join("config.json.tmp");
    let text = serde_json::to_string_pretty(cfg).map_err(|e| format!("配置序列化失败：{e}"))?;
    std::fs::write(&tmp, text).map_err(|e| format!("写入配置失败：{e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("保存配置失败：{e}"))?;
    Ok(())
}

pub fn upload_limit_bytes(cfg: &AppConfig) -> u64 {
    cfg.upload_limit_mb.saturating_mul(1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_validation() {
        assert!(AppConfig::is_valid_hex(&random_hex(32), 32));
        assert!(!AppConfig::is_valid_hex("xyz", 32));
    }
}
