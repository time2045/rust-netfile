use crate::config::{ConflictMode, STAGING_DIR};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::Mutex as AsyncMutex;

pub const STAGING_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

pub fn valid_id(id: &str) -> bool {
    (6..=128).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UploadMeta {
    pub rel_path: String,
    pub current_path: String,
    pub total_size: u64,
    #[serde(default)]
    pub created: f64,
}

pub fn staging_dir(root: &Path) -> PathBuf {
    root.join(STAGING_DIR)
}

pub fn part_path(root: &Path, id: &str) -> PathBuf {
    staging_dir(root).join(format!("{id}.part"))
}

pub fn meta_path(root: &Path, id: &str) -> PathBuf {
    staging_dir(root).join(format!("{id}.json"))
}

pub fn load_meta(path: &Path) -> Option<UploadMeta> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_meta(path: &Path, meta: &UploadMeta) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    ));
    let text = serde_json::to_string(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

pub fn meta_matches(meta: &UploadMeta, rel: &str, current: &str, total: u64) -> bool {
    meta.rel_path == rel && meta.current_path == current && meta.total_size == total
}

pub fn purge_staging(root: &Path) -> u64 {
    let dir = staging_dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .map(|age| age > STAGING_TTL)
            .unwrap_or(false);
        if old && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    let _ = std::fs::remove_dir(&dir);
    removed
}

struct Inner {
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    reserved: Mutex<HashSet<String>>,
}

#[derive(Clone)]
pub struct UploadManager {
    inner: Arc<Inner>,
}

impl UploadManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                locks: Mutex::new(HashMap::new()),
                reserved: Mutex::new(HashSet::new()),
            }),
        }
    }

    pub fn lock_for(&self, id: &str) -> Arc<AsyncMutex<()>> {
        let mut guard = match self.inner.locks.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard.len() > 500 {
            guard.retain(|_, lock| Arc::strong_count(lock) > 1);
        }
        guard
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    pub fn is_reserved(&self, path: &Path) -> bool {
        let key = key_of(path);
        self.inner
            .reserved
            .lock()
            .map(|g| g.contains(&key))
            .unwrap_or(false)
    }

    pub fn reserve(&self, target: &Path, mode: ConflictMode) -> Option<Reservation> {
        let mut reserved = match self.inner.reserved.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match mode {
            ConflictMode::Overwrite => {
                let key = key_of(target);
                if reserved.contains(&key) {
                    return None;
                }
                reserved.insert(key.clone());
                Some(Reservation {
                    path: target.to_path_buf(),
                    key,
                    inner: self.inner.clone(),
                })
            }
            ConflictMode::Rename => {
                let mut candidate = target.to_path_buf();
                let stem = target
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".into());
                let ext = target
                    .extension()
                    .map(|s| format!(".{}", s.to_string_lossy()))
                    .unwrap_or_default();
                for i in 1..10_000u32 {
                    let key = key_of(&candidate);
                    if !candidate.exists() && !reserved.contains(&key) {
                        reserved.insert(key.clone());
                        return Some(Reservation {
                            path: candidate,
                            key,
                            inner: self.inner.clone(),
                        });
                    }
                    candidate = target.with_file_name(format!("{stem}({i}){ext}"));
                }
                None
            }
            ConflictMode::Ask => {
                let key = key_of(target);
                if target.exists() || reserved.contains(&key) {
                    return None;
                }
                reserved.insert(key.clone());
                Some(Reservation {
                    path: target.to_path_buf(),
                    key,
                    inner: self.inner.clone(),
                })
            }
        }
    }
}

impl Default for UploadManager {
    fn default() -> Self {
        Self::new()
    }
}

fn key_of(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

pub struct Reservation {
    path: PathBuf,
    key: String,
    inner: Arc<Inner>,
}

impl Reservation {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.inner.reserved.lock() {
            guard.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_ids() {
        assert!(valid_id("u0123456789abcdef"));
        assert!(valid_id("abcDEF_123-456"));
        assert!(!valid_id("short"));
        assert!(!valid_id("bad/id"));
    }
}
