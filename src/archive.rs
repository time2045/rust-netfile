use crate::config::ZipCompress;
use crate::paths::{is_link_like, is_system_dir};
use crate::tasks::Task;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const MAX_ENTRIES: usize = 200_000;

pub struct ZipSpec {
    pub path: PathBuf,
    pub prefix: String,
}

struct Entry {
    abs: PathBuf,
    name: String,
    is_dir: bool,
    size: u64,
    mtime: SystemTime,
}

fn zip_datetime(time: SystemTime) -> zip::DateTime {
    let dt: chrono::DateTime<chrono::Local> = time.into();
    use chrono::{Datelike, Timelike};
    zip::DateTime::from_date_and_time(
        dt.year().clamp(1980, 2107) as u16,
        dt.month() as u8,
        dt.day() as u8,
        dt.hour() as u8,
        dt.minute() as u8,
        dt.second() as u8,
    )
    .unwrap_or_else(|_| zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).unwrap())
}

fn collect(specs: &[ZipSpec]) -> Result<Vec<Entry>, String> {
    let mut entries: Vec<Entry> = Vec::new();
    for spec in specs {
        let meta = fs::symlink_metadata(&spec.path)
            .map_err(|e| format!("无法读取 {}：{e}", spec.path.display()))?;
        if is_link_like(&meta) {
            continue;
        }
        let prefix = spec.prefix.trim_matches('/');
        if meta.is_file() {
            let name = if prefix.is_empty() {
                spec.path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".into())
            } else {
                prefix.to_string()
            };
            entries.push(Entry {
                abs: spec.path.clone(),
                name,
                is_dir: false,
                size: meta.len(),
                mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
            continue;
        }
        if !meta.is_dir() {
            continue;
        }
        if !prefix.is_empty() {
            entries.push(Entry {
                abs: spec.path.clone(),
                name: format!("{prefix}/"),
                is_dir: true,
                size: 0,
                mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
        for entry in WalkDir::new(&spec.path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                e.depth() == 0
                    || e.file_name()
                        .to_str()
                        .map(|n| !is_system_dir(n))
                        .unwrap_or(true)
            })
            .flatten()
        {
            if entry.depth() == 0 {
                continue;
            }
            let Ok(emeta) = entry.metadata() else {
                continue;
            };
            if is_link_like(&emeta) {
                continue;
            }
            let rel = match entry.path().strip_prefix(&spec.path) {
                Ok(rel) => rel
                    .components()
                    .filter_map(|c| match c {
                        std::path::Component::Normal(v) => Some(v.to_string_lossy().to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("/"),
                Err(_) => continue,
            };
            if rel.is_empty() {
                continue;
            }
            let name = if prefix.is_empty() {
                rel
            } else {
                format!("{prefix}/{rel}")
            };
            if emeta.is_dir() {
                entries.push(Entry {
                    abs: entry.path().to_path_buf(),
                    name: format!("{name}/"),
                    is_dir: true,
                    size: 0,
                    mtime: emeta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                });
            } else if emeta.is_file() {
                entries.push(Entry {
                    abs: entry.path().to_path_buf(),
                    name,
                    is_dir: false,
                    size: emeta.len(),
                    mtime: emeta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                });
            }
            if entries.len() > MAX_ENTRIES {
                return Err("打包项目过多（超过 20 万个条目）".into());
            }
        }
    }
    Ok(entries)
}

pub fn build_zip(
    specs: &[ZipSpec],
    zip_path: &Path,
    compress: ZipCompress,
    task: &Task,
) -> Result<u64, String> {
    let entries = collect(specs)?;
    let total: u64 = entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
    task.set_total(total);
    let file = fs::File::create(zip_path).map_err(|e| format!("无法创建临时压缩包：{e}"))?;
    let mut writer = ZipWriter::new(file);
    let mut buffer = vec![0u8; 256 * 1024];
    for entry in entries {
        let options = SimpleFileOptions::default()
            .compression_method(if entry.is_dir {
                zip::CompressionMethod::Stored
            } else {
                compress.method()
            })
            .compression_level(if entry.is_dir { None } else { compress.level() })
            .last_modified_time(zip_datetime(entry.mtime))
            .unix_permissions(if entry.is_dir { 0o755 } else { 0o644 })
            .large_file(!entry.is_dir && entry.size > 4 * 1024 * 1024 * 1024);
        if entry.is_dir {
            writer
                .add_directory(&entry.name, options)
                .map_err(|e| format!("写入压缩包目录失败：{e}"))?;
            continue;
        }
        writer
            .start_file(&entry.name, options)
            .map_err(|e| format!("写入压缩包条目失败：{e}"))?;
        let mut input = fs::File::open(&entry.abs)
            .map_err(|e| format!("读取文件失败 {}：{e}", entry.abs.display()))?;
        loop {
            let n = input
                .read(&mut buffer)
                .map_err(|e| format!("读取文件失败：{e}"))?;
            if n == 0 {
                break;
            }
            writer
                .write_all(&buffer[..n])
                .map_err(|e| format!("写入压缩包失败：{e}"))?;
            task.add_done(n as u64);
        }
    }
    let file = writer
        .finish()
        .map_err(|e| format!("完成压缩包失败：{e}"))?;
    file.sync_all().ok();
    let size = fs::metadata(zip_path).map(|m| m.len()).unwrap_or(0);
    Ok(size)
}

pub fn unique_top_names(sources: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut used: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for src in sources {
        let file_name = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "item".into());
        let stem = src
            .file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| file_name.clone());
        let ext = src
            .extension()
            .map(|n| format!(".{}", n.to_string_lossy()))
            .unwrap_or_default();
        let mut candidate = file_name.clone();
        let mut index = 1;
        while used
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&candidate))
        {
            candidate = format!("{stem}({index}){ext}");
            index += 1;
        }
        used.push(candidate.clone());
        out.push((src.clone(), candidate));
    }
    out
}
