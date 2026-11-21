use crate::error::{ApiError, ApiResult};
use crate::fsops;
use crate::paths::{join_rel, resolve_existing, resolve_for_create, same_file, validate_name};
use crate::recycle;
use crate::server::{get_str, get_str_opt, json_value, shared};
use crate::state::SharedState;
use axum::body::Bytes;
use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "项目".into())
}

fn ensure_not_root(root: &Path, target: &Path) -> Result<(), ApiError> {
    if target == root {
        return Err(ApiError::bad("不允许操作共享根目录"));
    }
    Ok(())
}

pub async fn mkdir(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let current = get_str_opt(&data, "current_path").unwrap_or_default();
    let name = get_str(&data, "name")?;
    let name = name.trim();
    validate_name(name).map_err(ApiError::bad)?;
    let root = state.root();
    let target = resolve_for_create(&root, &join_rel(&current, name))?;
    if target.exists() {
        return Err(ApiError::bad("目标位置已存在同名项目"));
    }
    std::fs::create_dir_all(&target).map_err(|e| ApiError::internal(format!("创建失败：{e}")))?;
    state.dir_sizes.invalidate();
    state.log.line(&format!("创建文件夹：{}", name));
    Ok(Json(json!({ "success": true })))
}

pub async fn rename(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let src_rel = get_str(&data, "path")?;
    let new_name = get_str(&data, "new_name")?;
    let new_name = new_name.trim();
    validate_name(new_name).map_err(ApiError::bad)?;
    let root = state.root();
    let src = resolve_existing(&root, &src_rel)?;
    ensure_not_root(&root, &src)?;
    let dest = src.with_file_name(new_name);
    if dest.exists() && !same_file(&src, &dest) {
        return Err(ApiError::bad(format!(
            "重命名失败：目标位置已存在同名项目（{new_name}）"
        )));
    }
    if src == dest {
        return Ok(Json(json!({ "success": true })));
    }
    let src2 = src.clone();
    let dest2 = dest.clone();
    tokio::task::spawn_blocking(move || std::fs::rename(&src2, &dest2))
        .await
        .map_err(|e| ApiError::internal(format!("重命名失败：{e}")))?
        .map_err(|e| ApiError::bad(format!("重命名失败：{e}")))?;
    state.dir_sizes.invalidate();
    state
        .log
        .line(&format!("重命名：{} → {}", name_of(&src), new_name));
    Ok(Json(json!({ "success": true })))
}

pub async fn move_one(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let src_rel = get_str(&data, "src")?;
    let dest_rel = get_str_opt(&data, "dest_dir").unwrap_or_default();
    let root = state.root();
    let src = resolve_existing(&root, &src_rel)?;
    ensure_not_root(&root, &src)?;
    let dest_dir = resolve_existing(&root, &dest_rel)?;
    if !dest_dir.is_dir() {
        return Err(ApiError::bad("目标不是文件夹"));
    }
    if src.is_dir() && dest_dir.starts_with(&src) {
        return Err(ApiError::bad(format!(
            "不能移动 [{}] 到其自身或子目录中",
            name_of(&src)
        )));
    }
    let target = dest_dir.join(name_of(&src));
    if target == src {
        return Err(ApiError::bad(format!(
            "[{}] 已在目标文件夹中",
            name_of(&src)
        )));
    }
    if target.exists() {
        return Err(ApiError::bad(format!(
            "移动失败：目标位置已存在同名项目（{}）",
            name_of(&target)
        )));
    }
    let src2 = src.clone();
    let target2 = target.clone();
    tokio::task::spawn_blocking(move || fsops::move_path(&src2, &target2))
        .await
        .map_err(|e| ApiError::internal(format!("移动失败：{e}")))?
        .map_err(|e| ApiError::bad(format!("移动失败：{e}")))?;
    state.dir_sizes.invalidate();
    state.log.line(&format!(
        "移动：{} → {}",
        name_of(&src),
        crate::paths::rel_string(&root, &dest_dir)
    ));
    Ok(Json(json!({ "success": true })))
}

fn collect_targets(
    root: &Path,
    srcs: &[String],
    dest_dir: &Path,
) -> Result<Vec<(PathBuf, PathBuf)>, ApiError> {
    if srcs.is_empty() {
        return Err(ApiError::bad("未选择任何项目"));
    }
    let mut out = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for rel in srcs {
        let src = resolve_existing(root, rel)?;
        ensure_not_root(root, &src)?;
        if src.is_dir() && dest_dir.starts_with(&src) {
            return Err(ApiError::bad(format!(
                "不能移动或复制 [{}] 到其自身或子目录中",
                name_of(&src)
            )));
        }
        let name = name_of(&src);
        let low = name.to_lowercase();
        if names.contains(&low) {
            return Err(ApiError::bad(format!("选中的项目存在同名冲突：{name}")));
        }
        names.push(low);
        let target = dest_dir.join(&name);
        if target == src {
            return Err(ApiError::bad(format!("[{name}] 已在目标文件夹中")));
        }
        if target.exists() {
            return Err(ApiError::bad(format!("目标位置已存在同名项目（{name}）")));
        }
        out.push((src, target));
    }
    Ok(out)
}

pub async fn move_many(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let dest_rel = get_str(&data, "dest_dir")?;
    let srcs: Vec<String> = data
        .get("srcs")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let root = state.root();
    let dest_dir = resolve_existing(&root, &dest_rel)?;
    if !dest_dir.is_dir() {
        return Err(ApiError::bad("目标不是文件夹"));
    }
    let targets = collect_targets(&root, &srcs, &dest_dir)?;
    let task = state
        .tasks
        .create("move", format!("正在移动 {} 个项目 ...", targets.len()));
    let state_t = state.clone();
    let task_t = task.clone();
    tokio::task::spawn_blocking(move || {
        let total: u64 = targets
            .iter()
            .map(|(src, _)| fsops::prewalk_bytes(src).1)
            .sum();
        task_t.set_total(total);
        let mut ok = 0;
        let mut fails = Vec::new();
        for (src, dst) in &targets {
            match fsops::move_any(src, dst, &task_t) {
                Ok(_) => {
                    ok += 1;
                    state_t.log.line(&format!("移动：{} 完成", name_of(src)));
                }
                Err(err) => fails.push(format!("{}：{err}", name_of(src))),
            }
        }
        state_t.dir_sizes.invalidate();
        let message = if fails.is_empty() {
            format!("移动完成：共 {ok} 个")
        } else {
            format!(
                "移动完成：成功 {ok} 个，失败 {} 个（{}）",
                fails.len(),
                fails.iter().take(3).cloned().collect::<Vec<_>>().join("；")
            )
        };
        if ok == 0 && !fails.is_empty() {
            task_t.finish_failed(message);
        } else {
            task_t.finish_done(message, None);
        }
    });
    Ok(Json(json!({ "success": true, "task_id": task.id })))
}

pub async fn copy(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let src_rel = get_str(&data, "src")?;
    let dest_rel = get_str_opt(&data, "dest_dir").unwrap_or_default();
    let root = state.root();
    let src = resolve_existing(&root, &src_rel)?;
    ensure_not_root(&root, &src)?;
    let dest_dir = resolve_existing(&root, &dest_rel)?;
    if !dest_dir.is_dir() {
        return Err(ApiError::bad("目标不是文件夹"));
    }
    if src.is_dir() && dest_dir.starts_with(&src) {
        return Err(ApiError::bad(format!(
            "不能复制 [{}] 到其自身或子目录中",
            name_of(&src)
        )));
    }
    let target = dest_dir.join(name_of(&src));
    if target == src {
        return Err(ApiError::bad(format!(
            "[{}] 已在目标文件夹中",
            name_of(&src)
        )));
    }
    if target.exists() {
        return Err(ApiError::bad(format!(
            "目标位置已存在同名项目（{}）",
            name_of(&target)
        )));
    }
    let task = state
        .tasks
        .create("copy", format!("正在复制 {} ...", name_of(&src)));
    let state_t = state.clone();
    let task_t = task.clone();
    let src2 = src.clone();
    let target2 = target.clone();
    tokio::task::spawn_blocking(move || {
        let (_, total) = fsops::prewalk_bytes(&src2);
        task_t.set_total(total);
        match fsops::copy_any(&src2, &target2, &task_t) {
            Ok(_) => {
                state_t.dir_sizes.invalidate();
                state_t.log.line(&format!("复制：{} 完成", name_of(&src2)));
                task_t.finish_done("复制完成", None);
            }
            Err(err) => {
                let _ = remove_partial(&target2);
                task_t.finish_failed(format!("复制失败：{err}"));
            }
        }
    });
    Ok(Json(json!({ "success": true, "task_id": task.id })))
}

fn remove_partial(path: &Path) {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub async fn copy_many(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let dest_rel = get_str(&data, "dest_dir")?;
    let srcs: Vec<String> = data
        .get("srcs")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let root = state.root();
    let dest_dir = resolve_existing(&root, &dest_rel)?;
    if !dest_dir.is_dir() {
        return Err(ApiError::bad("目标不是文件夹"));
    }
    let targets = collect_targets(&root, &srcs, &dest_dir)?;
    let task = state
        .tasks
        .create("copy", format!("正在复制 {} 个项目 ...", targets.len()));
    let state_t = state.clone();
    let task_t = task.clone();
    tokio::task::spawn_blocking(move || {
        let total: u64 = targets
            .iter()
            .map(|(src, _)| fsops::prewalk_bytes(src).1)
            .sum();
        task_t.set_total(total);
        let mut ok = 0;
        let mut fails = Vec::new();
        for (src, dst) in &targets {
            match fsops::copy_any(src, dst, &task_t) {
                Ok(_) => {
                    ok += 1;
                    state_t.log.line(&format!("复制：{} 完成", name_of(src)));
                }
                Err(err) => {
                    remove_partial(dst);
                    fails.push(format!("{}：{err}", name_of(src)));
                }
            }
        }
        state_t.dir_sizes.invalidate();
        let message = if fails.is_empty() {
            format!("复制完成：共 {ok} 个")
        } else {
            format!(
                "复制完成：成功 {ok} 个，失败 {} 个（{}）",
                fails.len(),
                fails.iter().take(3).cloned().collect::<Vec<_>>().join("；")
            )
        };
        if ok == 0 && !fails.is_empty() {
            task_t.finish_failed(message);
        } else {
            task_t.finish_done(message, None);
        }
    });
    Ok(Json(json!({ "success": true, "task_id": task.id })))
}

pub async fn delete(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let rel = get_str(&data, "path")?;
    let root = state.root();
    let src = resolve_existing(&root, &rel)?;
    ensure_not_root(&root, &src)?;
    let task = state
        .tasks
        .create("delete", format!("正在删除 {} ...", name_of(&src)));
    let state_t = state.clone();
    let task_t = task.clone();
    let rel2 = rel.clone();
    tokio::task::spawn_blocking(move || match recycle::move_to_recycle(&state_t, &src) {
        Ok(_) => {
            state_t.dir_sizes.invalidate();
            state_t.log.line(&format!("已移入回收站：{rel2}"));
            task_t.finish_done("删除完成（已移入回收站）", None);
        }
        Err(err) => task_t.finish_failed(format!("删除失败：{err}")),
    });
    Ok(Json(json!({ "success": true, "task_id": task.id })))
}

pub async fn delete_many(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let paths: Vec<String> = data
        .get("paths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if paths.is_empty() {
        return Err(ApiError::bad("未选择任何项目"));
    }
    let root = state.root();
    let mut targets = Vec::new();
    for rel in &paths {
        let path = resolve_existing(&root, rel)?;
        ensure_not_root(&root, &path)?;
        targets.push(path);
    }
    let task = state
        .tasks
        .create("delete", format!("正在删除 {} 个项目 ...", targets.len()));
    let state_t = state.clone();
    let task_t = task.clone();
    tokio::task::spawn_blocking(move || {
        let mut ok = 0;
        let mut fails = Vec::new();
        for target in &targets {
            match recycle::move_to_recycle(&state_t, target) {
                Ok(_) => {
                    ok += 1;
                    state_t
                        .log
                        .line(&format!("已移入回收站：{}", name_of(target)));
                }
                Err(err) => fails.push(format!("{}：{err}", name_of(target))),
            }
        }
        state_t.dir_sizes.invalidate();
        let message = if fails.is_empty() {
            format!("删除完成：共 {ok} 个（已移入回收站）")
        } else {
            format!("删除完成：成功 {ok} 个，失败 {} 个", fails.len())
        };
        if ok == 0 && !fails.is_empty() {
            task_t.finish_failed(message);
        } else {
            task_t.finish_done(message, None);
        }
    });
    Ok(Json(json!({ "success": true, "task_id": task.id })))
}

pub async fn check_conflicts(state: State<SharedState>, body: Bytes) -> ApiResult<Json<Value>> {
    let state = shared(state);
    let data = json_value(&body)?;
    let current = get_str_opt(&data, "current_path").unwrap_or_default();
    let rels: Vec<String> = data
        .get("rel_paths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let root = state.root();
    let mut conflicts = Vec::new();
    for rel in rels {
        let target = resolve_for_create(&root, &join_rel(&current, &rel))?;
        if target.exists() {
            conflicts.push(rel);
        }
    }
    Ok(Json(json!({ "success": true, "conflicts": conflicts })))
}
