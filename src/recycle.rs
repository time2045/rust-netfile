use crate::config::RECYCLE_DIR;
use crate::paths::{auto_rename, is_link_like, resolve_for_create, system_time_secs};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Serialize)]
pub struct RecycleEntry {
    pub name: String,
    pub orig: String,
    pub deleted_ts: f64,
    pub size: u64,
    pub is_dir: bool,
    pub days_left: i64,
}

#[derive(Clone, Deserialize, Serialize, Default)]
struct MetaItem {
    #[serde(default)]
    orig: String,
    #[serde(default)]
    time: f64,
}

type Meta = HashMap<String, MetaItem>;

fn recycle_dir(root: &Path) -> PathBuf {
    root.join(RECYCLE_DIR)
}

fn meta_path(root: &Path) -> PathBuf {
    recycle_dir(root).join("meta.json")
}

fn lock(state: &AppState) -> std::sync::MutexGuard<'_, ()> {
    state
        .recycle_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn load_meta(root: &Path) -> Meta {
    std::fs::read_to_string(meta_path(root))
        .ok()
        .and_then(|text| serde_json::from_str::<Meta>(&text).ok())
        .unwrap_or_default()
}

fn save_meta(root: &Path, meta: &Meta) {
    let dir = recycle_dir(root);
    let _ = std::fs::create_dir_all(&dir);
    let path = meta_path(root);
    let tmp = dir.join("meta.json.tmp");
    if let Ok(text) = serde_json::to_string_pretty(meta) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

fn strip_stamp(name: &str) -> String {
    let mut base = name.to_string();
    if let Some(pos) = base.rfind(" (") {
        if base.ends_with(')') {
            base.truncate(pos);
        }
    }
    if base.len() < 16 {
        return base;
    }
    let tail = &base[base.len() - 16..];
    let bytes = tail.as_bytes();
    let valid = bytes[0] == b'_'
        && bytes[1..9].iter().all(|b| b.is_ascii_digit())
        && bytes[9] == b'_'
        && bytes[10..16].iter().all(|b| b.is_ascii_digit());
    if valid {
        base.truncate(base.len() - 16);
    }
    base
}

fn valid_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.eq_ignore_ascii_case("meta.json")
        && !name.eq_ignore_ascii_case("meta.json.tmp")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

pub fn move_to_recycle(state: &AppState, src: &Path) -> Result<PathBuf, String> {
    let _guard = lock(state);
    let root = state.root();
    let dir = recycle_dir(&root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建回收站目录：{e}"))?;
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| "无法获取项目名称".to_string())?;
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let dest = auto_rename(&dir.join(format!("{name}_{stamp}")));
    std::fs::rename(src, &dest).map_err(|e| format!("移入回收站失败：{e}"))?;
    let now = SystemTime::now();
    let _ = filetime::set_file_mtime(&dest, filetime::FileTime::from_system_time(now));
    let rel = crate::paths::rel_string(&root, src);
    let mut meta = load_meta(&root);
    meta.insert(
        dest.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        MetaItem {
            orig: rel,
            time: system_time_secs(now),
        },
    );
    save_meta(&root, &meta);
    Ok(dest)
}

pub fn scan(state: &AppState) -> Vec<RecycleEntry> {
    let _guard = lock(state);
    let root = state.root();
    let dir = recycle_dir(&root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let meta = load_meta(&root);
    let now = system_time_secs(SystemTime::now());
    let days = state.cfg().recycle_days as f64;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !valid_entry_name(&name) {
            continue;
        }
        let Ok(link_meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if is_link_like(&link_meta) {
            continue;
        }
        let is_dir = link_meta.is_dir();
        let item = meta.get(&name);
        let orig = item
            .map(|m| m.orig.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| strip_stamp(&name));
        let deleted_ts = item
            .map(|m| m.time)
            .filter(|t| *t > 0.0)
            .unwrap_or_else(|| link_meta.modified().map(system_time_secs).unwrap_or(now));
        let remaining = days * 86400.0 - (now - deleted_ts);
        let days_left = (remaining / 86400.0).ceil().max(0.0) as i64;
        out.push(RecycleEntry {
            name,
            orig,
            deleted_ts,
            size: if is_dir { 0 } else { link_meta.len() },
            is_dir,
            days_left,
        });
    }
    out.sort_by(|a, b| {
        b.deleted_ts
            .partial_cmp(&a.deleted_ts)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn entry_path(root: &Path, name: &str) -> Option<PathBuf> {
    if !valid_entry_name(name) {
        return None;
    }
    let path = recycle_dir(root).join(name);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

pub fn restore(state: &AppState, names: &[String]) -> (u64, Vec<String>) {
    let _guard = lock(state);
    let root = state.root();
    let mut meta = load_meta(&root);
    let mut ok = 0;
    let mut fails = Vec::new();
    let mut changed = false;
    for name in names {
        let Some(src) = entry_path(&root, name) else {
            fails.push(format!("{name}：条目不存在"));
            continue;
        };
        let Ok(link_meta) = std::fs::symlink_metadata(&src) else {
            fails.push(format!("{name}：无法读取条目"));
            continue;
        };
        if is_link_like(&link_meta) {
            fails.push(format!("{name}：不支持还原链接条目"));
            continue;
        }
        let orig = meta
            .get(name)
            .map(|m| m.orig.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| strip_stamp(name));
        let target = match resolve_for_create(&root, &orig) {
            Ok(path) => path,
            Err(err) => {
                fails.push(format!("{name}：{}", err.message));
                continue;
            }
        };
        let target = if target == root {
            fails.push(format!("{name}：不允许还原到共享根目录"));
            continue;
        } else if target.exists() {
            auto_rename(&target)
        } else {
            target
        };
        if let Some(parent) = target.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                fails.push(format!("{name}：无法创建目录（{err}）"));
                continue;
            }
        }
        match std::fs::rename(&src, &target) {
            Ok(_) => {
                meta.remove(name);
                changed = true;
                ok += 1;
                state.log.line(&format!(
                    "回收站还原：{name} → {}",
                    crate::paths::rel_string(&root, &target)
                ));
            }
            Err(err) => fails.push(format!("{name}：{err}")),
        }
    }
    if changed {
        save_meta(&root, &meta);
        state.dir_sizes.invalidate();
    }
    (ok, fails)
}

pub fn purge(state: &AppState, names: &[String]) -> (u64, Vec<String>) {
    let _guard = lock(state);
    let root = state.root();
    let mut meta = load_meta(&root);
    let mut ok = 0;
    let mut fails = Vec::new();
    let mut changed = false;
    for name in names {
        let Some(path) = entry_path(&root, name) else {
            fails.push(format!("{name}：条目不存在"));
            continue;
        };
        let Ok(link_meta) = std::fs::symlink_metadata(&path) else {
            fails.push(format!("{name}：无法读取条目"));
            continue;
        };
        if is_link_like(&link_meta) {
            fails.push(format!("{name}：链接条目不删除"));
            continue;
        }
        let result = if link_meta.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(_) => {
                meta.remove(name);
                changed = true;
                ok += 1;
                state.log.line(&format!("回收站永久删除：{name}"));
            }
            Err(err) => fails.push(format!("{name}：{err}")),
        }
    }
    if changed {
        save_meta(&root, &meta);
    }
    (ok, fails)
}

pub fn clear(state: &AppState) -> (u64, Vec<String>) {
    let names: Vec<String> = scan(state).into_iter().map(|e| e.name).collect();
    purge(state, &names)
}

pub fn purge_expired(state: &AppState) -> u64 {
    let _guard = lock(state);
    let root = state.root();
    let dir = recycle_dir(&root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let mut meta = load_meta(&root);
    let now = system_time_secs(SystemTime::now());
    let ttl = state.cfg().recycle_days as f64 * 86400.0;
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !valid_entry_name(&name) {
            continue;
        }
        let Ok(link_meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if is_link_like(&link_meta) {
            continue;
        }
        let deleted = meta
            .get(&name)
            .map(|m| m.time)
            .filter(|t| *t > 0.0)
            .unwrap_or_else(|| link_meta.modified().map(system_time_secs).unwrap_or(now));
        if now - deleted <= ttl {
            continue;
        }
        let result = if link_meta.is_dir() {
            std::fs::remove_dir_all(entry.path())
        } else {
            std::fs::remove_file(entry.path())
        };
        if result.is_ok() {
            meta.remove(&name);
            removed += 1;
        }
    }
    if removed > 0 {
        save_meta(&root, &meta);
        state
            .log
            .line(&format!("回收站自动清理：清除 {removed} 个超期条目"));
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::logging::LogStore;
    use crate::state::AppState;

    #[test]
    fn recycle_roundtrip() {
        let base =
            std::env::temp_dir().join(format!("nb-recycle-{}", crate::config::random_hex(8)));
        let data_dir = base.join("data");
        let root = base.join("files");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        let canonical = std::fs::canonicalize(&root).unwrap();
        let mut cfg = AppConfig::default();
        cfg.root_dir = canonical.to_string_lossy().to_string();
        let log = LogStore::new(&data_dir);
        let state = AppState::new(data_dir, cfg, log, None);

        let file = root.join("回收测试.txt");
        std::fs::write(&file, b"content").unwrap();
        move_to_recycle(&state, &file).unwrap();
        assert!(!file.exists());
        let entries = scan(&state);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].orig, "回收测试.txt");

        let (ok, fails) = restore(&state, &[entries[0].name.clone()]);
        assert_eq!(ok, 1, "{fails:?}");
        assert!(file.exists());

        move_to_recycle(&state, &file).unwrap();
        let entries = scan(&state);
        let (ok, fails) = purge(&state, &[entries[0].name.clone()]);
        assert_eq!(ok, 1, "{fails:?}");
        assert!(scan(&state).is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn stamp_stripping() {
        assert_eq!(strip_stamp("报告_20240101_120000"), "报告");
        assert_eq!(strip_stamp("报告_20240101_120000 (1)"), "报告");
        assert_eq!(strip_stamp("普通文件"), "普通文件");
    }

    #[test]
    fn entry_name_validation() {
        assert!(valid_entry_name("a_20240101_120000"));
        assert!(!valid_entry_name("a/b"));
        assert!(!valid_entry_name(".."));
        assert!(!valid_entry_name("meta.json"));
    }
}
