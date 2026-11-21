use crate::paths::{is_link_like, is_system_dir};
use crate::tasks::Task;
use std::fs;
use std::io::Write;
use std::path::Path;
use walkdir::WalkDir;

pub fn prewalk_bytes(src: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let meta = match fs::symlink_metadata(src) {
        Ok(meta) => meta,
        Err(_) => return (0, 0),
    };
    if is_link_like(&meta) {
        return (0, 0);
    }
    if meta.is_file() {
        return (1, meta.len());
    }
    if !meta.is_dir() {
        return (0, 0);
    }
    for entry in WalkDir::new(src)
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
        let Ok(meta) = entry.metadata() else { continue };
        if is_link_like(&meta) || !meta.is_file() {
            continue;
        }
        files += 1;
        bytes = bytes.saturating_add(meta.len());
    }
    (files, bytes)
}

fn copy_file(src: &Path, dst: &Path, progress: Option<&Task>) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("无法创建目录 {}：{e}", parent.display()))?;
    }
    let meta = fs::metadata(src).map_err(|e| format!("读取源文件失败：{e}"))?;
    let mut input = fs::File::open(src).map_err(|e| format!("打开源文件失败：{e}"))?;
    let mut output = fs::File::create(dst).map_err(|e| format!("创建目标文件失败：{e}"))?;
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        use std::io::Read;
        let n = input
            .read(&mut buffer)
            .map_err(|e| format!("读取源文件失败：{e}"))?;
        if n == 0 {
            break;
        }
        output
            .write_all(&buffer[..n])
            .map_err(|e| format!("写入目标文件失败：{e}"))?;
        if let Some(task) = progress {
            task.add_done(n as u64);
        }
    }
    output.flush().ok();
    if let Ok(mtime) = meta.modified() {
        let _ = filetime::set_file_mtime(dst, filetime::FileTime::from_system_time(mtime));
    }
    Ok(())
}

fn copy_any_inner(src: &Path, dst: &Path, progress: Option<&Task>) -> Result<(), String> {
    let meta = fs::symlink_metadata(src).map_err(|e| format!("无法读取源项目：{e}"))?;
    if is_link_like(&meta) {
        return Err("不支持复制链接条目".into());
    }
    if meta.is_file() {
        return copy_file(src, dst, progress);
    }
    if !meta.is_dir() {
        return Err("不支持的项目类型".into());
    }
    fs::create_dir_all(dst).map_err(|e| format!("无法创建目标文件夹：{e}"))?;
    for entry in WalkDir::new(src)
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
        let path = entry.path();
        let Ok(emeta) = entry.metadata() else {
            continue;
        };
        if is_link_like(&emeta) {
            continue;
        }
        let rel = match path.strip_prefix(src) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dst.join(rel);
        if emeta.is_dir() {
            fs::create_dir_all(&target)
                .map_err(|e| format!("无法创建目录 {}：{e}", target.display()))?;
        } else if emeta.is_file() {
            copy_file(path, &target, progress)?;
        }
    }
    if let Ok(mtime) = meta.modified() {
        let _ = filetime::set_file_mtime(dst, filetime::FileTime::from_system_time(mtime));
    }
    Ok(())
}

pub fn copy_any(src: &Path, dst: &Path, task: &Task) -> Result<(), String> {
    copy_any_inner(src, dst, Some(task))
}

fn remove_path(src: &Path, meta: &fs::Metadata) -> Result<(), String> {
    let removed = if meta.is_dir() {
        fs::remove_dir_all(src)
    } else {
        fs::remove_file(src)
    };
    removed.map_err(|e| format!("删除源项目失败：{e}"))
}

pub fn move_any(src: &Path, dst: &Path, task: &Task) -> Result<(), String> {
    let meta = fs::symlink_metadata(src).map_err(|e| format!("无法读取源项目：{e}"))?;
    if is_link_like(&meta) {
        return Err("不支持移动链接条目".into());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建目标目录：{e}"))?;
    }
    match fs::rename(src, dst) {
        Ok(_) => {
            if meta.is_file() {
                task.add_done(meta.len());
            }
            Ok(())
        }
        Err(_) => {
            copy_any_inner(src, dst, Some(task))?;
            remove_path(src, &meta)?;
            Ok(())
        }
    }
}

pub fn move_path(src: &Path, dst: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(src).map_err(|e| format!("无法读取源项目：{e}"))?;
    if is_link_like(&meta) {
        return Err("不支持移动链接条目".into());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建目标目录：{e}"))?;
    }
    match fs::rename(src, dst) {
        Ok(_) => Ok(()),
        Err(_) => {
            copy_any_inner(src, dst, None)?;
            remove_path(src, &meta)
        }
    }
}
