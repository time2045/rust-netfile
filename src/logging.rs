use chrono::Local;
use serde::Serialize;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const MAX_ENTRIES: usize = 5000;
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone, Serialize)]
pub struct AccessEntry {
    pub seq: u64,
    pub time: String,
    pub ip: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub ms: u64,
}

#[derive(Clone, Serialize)]
pub struct AuditEntry {
    pub seq: u64,
    pub time: String,
    pub name: String,
    pub size: u64,
    pub ip: String,
    pub result: String,
}

#[derive(Serialize)]
pub struct LogPage<T> {
    pub items: Vec<T>,
    pub total: usize,
    pub total_pages: usize,
    pub page: usize,
    pub page_size: usize,
    #[serde(skip)]
    pub _marker: std::marker::PhantomData<T>,
}

pub struct LogStore {
    access: Mutex<VecDeque<AccessEntry>>,
    upload: Mutex<VecDeque<AuditEntry>>,
    download: Mutex<VecDeque<AuditEntry>>,
    seq: AtomicU64,
    file: Mutex<Option<File>>,
    file_path: PathBuf,
}

pub fn now_text() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn paginate<T: Clone>(
    items: &[T],
    page: usize,
    page_size: usize,
) -> (Vec<T>, usize, usize, usize) {
    let total = items.len();
    let page_size = page_size.clamp(1, 1000);
    let total_pages = ((total + page_size - 1) / page_size).max(1);
    let page = page.clamp(1, total_pages);
    let start = (page - 1) * page_size;
    let end = (start + page_size).min(total);
    (items[start..end].to_vec(), total, total_pages, page)
}

impl LogStore {
    pub fn new(data_dir: &Path) -> Self {
        let logs_dir = data_dir.join(crate::config::LOGS_DIR);
        let _ = std::fs::create_dir_all(&logs_dir);
        let file_path = logs_dir.join("server.log");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .ok();
        Self {
            access: Mutex::new(VecDeque::with_capacity(256)),
            upload: Mutex::new(VecDeque::with_capacity(256)),
            download: Mutex::new(VecDeque::with_capacity(256)),
            seq: AtomicU64::new(0),
            file: Mutex::new(file),
            file_path,
        }
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn line(&self, message: &str) {
        let text = format!("[{}] {}\n", now_text(), message);
        let mut guard = match self.file.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let Some(file) = guard.as_mut() else { return };
        let too_large = file
            .metadata()
            .map(|m| m.len() > LOG_MAX_BYTES)
            .unwrap_or(false);
        if too_large {
            drop(guard);
            self.rotate();
            let mut guard = match self.file.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let Some(file) = guard.as_mut() else { return };
            let _ = file.write_all(text.as_bytes());
            let _ = file.flush();
            return;
        }
        let _ = file.write_all(text.as_bytes());
        let _ = file.flush();
    }

    fn rotate(&self) {
        let backup = self.file_path.with_extension("log.1");
        let _ = std::fs::remove_file(&backup);
        let _ = std::fs::rename(&self.file_path, &backup);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)
            .ok();
        if let Ok(mut guard) = self.file.lock() {
            *guard = file;
        }
    }

    pub fn access(&self, ip: &str, method: &str, path: &str, status: u16, ms: u64) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = AccessEntry {
            seq,
            time: now_text(),
            ip: ip.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            status,
            ms,
        };
        if let Ok(mut guard) = self.access.lock() {
            if guard.len() >= MAX_ENTRIES {
                guard.pop_front();
            }
            guard.push_back(entry.clone());
        }
        self.line(&format!("{ip} \"{method} {path}\" {status} {ms}ms"));
    }

    pub fn audit(&self, kind: &str, name: &str, size: u64, ip: &str, result: &str) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = AuditEntry {
            seq,
            time: now_text(),
            name: name.chars().take(300).collect(),
            size,
            ip: ip.chars().take(64).collect(),
            result: result.chars().take(120).collect(),
        };
        let target = match kind {
            "upload" => &self.upload,
            "download" => &self.download,
            _ => return,
        };
        if let Ok(mut guard) = target.lock() {
            if guard.len() >= MAX_ENTRIES {
                guard.pop_front();
            }
            guard.push_back(entry);
        }
        self.line(&format!(
            "{} {} {} {} {}",
            if kind == "upload" {
                "UPLOAD"
            } else {
                "DOWNLOAD"
            },
            ip,
            name,
            crate::paths::format_size(size),
            result
        ));
    }

    pub fn access_page(&self, page: usize, page_size: usize) -> LogPage<AccessEntry> {
        let mut items: Vec<AccessEntry> = match self.access.lock() {
            Ok(g) => g.iter().cloned().collect(),
            Err(p) => p.into_inner().iter().cloned().collect(),
        };
        items.reverse();
        let (items, total, total_pages, page) = paginate(&items, page, page_size);
        LogPage {
            items,
            total,
            total_pages,
            page,
            page_size: page_size.clamp(1, 1000),
            _marker: std::marker::PhantomData,
        }
    }

    pub fn audit_page(&self, kind: &str, page: usize, page_size: usize) -> LogPage<AuditEntry> {
        let target = match kind {
            "upload" => &self.upload,
            _ => &self.download,
        };
        let mut items: Vec<AuditEntry> = match target.lock() {
            Ok(g) => g.iter().cloned().collect(),
            Err(p) => p.into_inner().iter().cloned().collect(),
        };
        items.reverse();
        let (items, total, total_pages, page) = paginate(&items, page, page_size);
        LogPage {
            items,
            total,
            total_pages,
            page,
            page_size: page_size.clamp(1, 1000),
            _marker: std::marker::PhantomData,
        }
    }

    pub fn clear_all(&self) {
        if let Ok(mut g) = self.access.lock() {
            g.clear();
        }
        if let Ok(mut g) = self.upload.lock() {
            g.clear();
        }
        if let Ok(mut g) = self.download.lock() {
            g.clear();
        }
        if let Ok(mut g) = self.file.lock() {
            if let Some(file) = g.as_mut() {
                let _ = file.set_len(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_clamps() {
        let items = vec![1, 2, 3, 4, 5];
        let (page, total, pages, current) = paginate(&items, 9, 2);
        assert_eq!(total, 5);
        assert_eq!(pages, 3);
        assert_eq!(current, 3);
        assert_eq!(page, vec![5]);
    }
}
