use crate::error::{ApiError, ApiResult};
use crate::paths::{format_size, format_time, is_system_dir, resolve_existing, system_time_secs};
use crate::state::{AppState, SharedState};
use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use walkdir::WalkDir;

const SEARCH_LIMIT: usize = 5000;
const FOLDER_LIMIT: usize = 10_000;

#[derive(Deserialize, Default)]
pub struct ListQuery {
    path: Option<String>,
    page: Option<String>,
    page_size: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    q: Option<String>,
}

#[derive(Serialize)]
pub struct ListItem {
    name: String,
    is_dir: bool,
    rel_path: String,
    size: String,
    size_bytes: Option<u64>,
    mtime: String,
    mtime_ts: f64,
}

#[derive(Serialize)]
struct Crumb {
    name: String,
    path: String,
}

#[derive(Serialize)]
pub struct ListResponse {
    success: bool,
    path: String,
    items: Vec<ListItem>,
    total: usize,
    page: usize,
    total_pages: usize,
    page_size: usize,
    sort: String,
    order: String,
    breadcrumbs: Vec<Crumb>,
    search: String,
    truncated: bool,
}

fn parse_page(text: Option<&String>) -> usize {
    text.and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1)
}

fn parse_page_size(text: Option<&String>) -> usize {
    text.and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, 1000)
}

fn make_item(
    root: &Path,
    path: &Path,
    is_dir: bool,
    meta: &std::fs::Metadata,
    rel: String,
) -> ListItem {
    let _ = root;
    ListItem {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        is_dir,
        rel_path: rel,
        size: if is_dir {
            String::new()
        } else {
            format_size(meta.len())
        },
        size_bytes: if is_dir { None } else { Some(meta.len()) },
        mtime: meta.modified().map(format_time).unwrap_or_default(),
        mtime_ts: meta.modified().map(system_time_secs).unwrap_or(0.0),
    }
}

fn collect_search(root: &Path, base: &Path, _base_rel: &str, query: &str) -> (Vec<ListItem>, bool) {
    let needle = query.to_lowercase();
    let mut items = Vec::new();
    let mut truncated = false;
    for entry in WalkDir::new(base)
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
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.to_lowercase().contains(&needle) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let rel = crate::paths::rel_string(root, entry.path());
        let is_dir = meta.is_dir();
        items.push(make_item(root, entry.path(), is_dir, &meta, rel));
        if items.len() >= SEARCH_LIMIT {
            truncated = true;
            break;
        }
    }
    (items, truncated)
}

fn collect_dir(root: &Path, base: &Path, base_rel: &str) -> Vec<ListItem> {
    let mut items = Vec::new();
    let Ok(entries) = std::fs::read_dir(base) else {
        return items;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if is_system_dir(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let rel = if base_rel.is_empty() {
            name.clone()
        } else {
            format!("{base_rel}/{name}")
        };
        let is_dir = meta.is_dir();
        items.push(make_item(root, &entry.path(), is_dir, &meta, rel));
    }
    items
}

pub async fn list(
    state: State<SharedState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<ListResponse>> {
    let state = shared_state(&state);
    let mut rel = query.path.clone().unwrap_or_default();
    rel = rel.replace('\\', "/");
    rel = rel.trim_matches('/').to_string();
    let page = parse_page(query.page.as_ref());
    let page_size = parse_page_size(query.page_size.as_ref());
    let sort = match query.sort.as_deref() {
        Some("size") => "size",
        Some("mtime") => "mtime",
        _ => "name",
    }
    .to_string();
    let order = match query.order.as_deref() {
        Some("desc") => "desc",
        _ => "asc",
    }
    .to_string();
    let search = query.q.clone().unwrap_or_default().trim().to_string();
    let root = state.root();
    let abs = resolve_existing(&root, &rel)?;
    if !abs.is_dir() {
        return Err(ApiError::bad("目标不是文件夹"));
    }
    let base_rel = rel.clone();
    let sort_key = sort.clone();
    let reverse = order == "desc";
    let search_key = search.clone();
    let root2 = root.clone();
    let abs2 = abs.clone();
    let state2 = state.clone();

    let (mut items, truncated) = tokio::task::spawn_blocking(move || {
        let (mut items, truncated) = if search_key.is_empty() {
            (collect_dir(&root2, &abs2, &base_rel), false)
        } else {
            collect_search(&root2, &abs2, &base_rel, &search_key)
        };
        if sort_key == "size" {
            for item in items.iter_mut().filter(|i| i.is_dir) {
                let path = root2.join(&item.rel_path);
                let (_count, size) = state2.dir_sizes.get(&path);
                item.size = format_size(size);
                item.size_bytes = Some(size);
            }
        }
        let value = |item: &ListItem| -> f64 {
            match sort_key.as_str() {
                "size" => item.size_bytes.unwrap_or(0) as f64,
                "mtime" => item.mtime_ts,
                _ => 0.0,
            }
        };
        if sort_key == "name" {
            items.sort_by(|a, b| {
                if a.is_dir != b.is_dir {
                    return b.is_dir.cmp(&a.is_dir);
                }
                let cmp = a.name.to_lowercase().cmp(&b.name.to_lowercase());
                if reverse {
                    cmp.reverse()
                } else {
                    cmp
                }
            });
        } else {
            items.sort_by(|a, b| {
                if a.is_dir != b.is_dir {
                    return b.is_dir.cmp(&a.is_dir);
                }
                let cmp = value(a)
                    .partial_cmp(&value(b))
                    .unwrap_or(std::cmp::Ordering::Equal);
                if reverse {
                    cmp.reverse()
                } else {
                    cmp
                }
            });
        }
        (items, truncated)
    })
    .await
    .map_err(|e| ApiError::internal(format!("读取目录失败：{e}")))?;

    let total = items.len();
    let total_pages = ((total + page_size - 1) / page_size).max(1);
    let page = page.min(total_pages);
    let start = (page - 1) * page_size;
    let end = (start + page_size).min(total);
    let mut page_items: Vec<ListItem> = if start < end {
        items.drain(start..end).collect()
    } else {
        Vec::new()
    };
    if sort != "size" {
        let state3 = state.clone();
        let root3 = root.clone();
        let fills: Vec<(usize, PathBuf)> = page_items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.is_dir)
            .map(|(idx, item)| (idx, root3.join(&item.rel_path)))
            .collect();
        if !fills.is_empty() {
            let sizes = tokio::task::spawn_blocking(move || {
                fills
                    .into_iter()
                    .map(|(idx, path)| (idx, state3.dir_sizes.get(&path)))
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            for (idx, (_count, size)) in sizes {
                if let Some(item) = page_items.get_mut(idx) {
                    item.size = format_size(size);
                    item.size_bytes = Some(size);
                }
            }
        }
    }

    let mut breadcrumbs = Vec::new();
    let mut current = String::new();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        if current.is_empty() {
            current = part.to_string();
        } else {
            current = format!("{current}/{part}");
        }
        breadcrumbs.push(Crumb {
            name: part.to_string(),
            path: current.clone(),
        });
    }

    Ok(Json(ListResponse {
        success: true,
        path: rel,
        items: page_items,
        total,
        page,
        total_pages,
        page_size,
        sort,
        order,
        breadcrumbs,
        search,
        truncated,
    }))
}

fn shared_state(state: &State<SharedState>) -> Arc<AppState> {
    state.0.clone()
}

#[derive(Deserialize, Default)]
pub struct FolderQuery {
    exclude: Option<String>,
}

#[derive(Serialize)]
struct FolderItem {
    path: String,
    name: String,
    depth: usize,
}

pub async fn folders(
    state: State<SharedState>,
    Query(query): Query<FolderQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let state = state.0.clone();
    let root = state.root();
    let excludes: Vec<String> = query
        .exclude
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().trim_matches('/').to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let root2 = root.clone();
    let result = tokio::task::spawn_blocking(move || -> (Vec<FolderItem>, bool) {
        let mut out = vec![FolderItem {
            path: String::new(),
            name: "根目录".into(),
            depth: 0,
        }];
        let mut truncated = false;
        let mut stack: Vec<(PathBuf, String, usize)> = vec![(root2.clone(), String::new(), 0)];
        while let Some((dir, rel_prefix, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut children: Vec<(String, PathBuf)> = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if is_system_dir(&name) {
                    continue;
                }
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() || file_type.is_symlink() {
                    continue;
                }
                let rel = if rel_prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{rel_prefix}/{name}")
                };
                let low = rel.to_lowercase();
                if excludes
                    .iter()
                    .any(|ex| low == *ex || low.starts_with(&format!("{ex}/")))
                {
                    continue;
                }
                children.push((rel, entry.path()));
            }
            children.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
            for (rel, path) in children.into_iter().rev() {
                let name = rel.rsplit('/').next().unwrap_or_default().to_string();
                let child_depth = depth + 1;
                out.push(FolderItem {
                    path: rel.clone(),
                    name,
                    depth: child_depth,
                });
                if out.len() >= FOLDER_LIMIT {
                    truncated = true;
                    return (out, truncated);
                }
                stack.push((path, rel, child_depth));
            }
        }
        (out, truncated)
    })
    .await
    .map_err(|e| ApiError::internal(format!("读取文件夹失败：{e}")))?;
    Ok(Json(json!({
        "success": true,
        "folders": result.0,
        "truncated": result.1,
    })))
}
