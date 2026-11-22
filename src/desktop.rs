use crate::config;
use crate::config::{ConflictMode, ZipCompress};
use crate::state::{AppState, SharedState, Snapshot};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, Wry};

static TRAY_READY: AtomicBool = AtomicBool::new(false);
static EXITING: AtomicBool = AtomicBool::new(false);
static LOCK_ITEMS: Mutex<Option<(MenuItem<Wry>, MenuItem<Wry>)>> = Mutex::new(None);

pub fn tray_ready() -> bool {
    TRAY_READY.load(Ordering::SeqCst)
}

pub fn is_exiting() -> bool {
    EXITING.load(Ordering::SeqCst)
}

fn show_error(message: &str) {
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("NB 文件管理器")
        .set_description(message)
        .show();
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn sync_lock(locked: bool) {
    if let Ok(guard) = LOCK_ITEMS.lock() {
        if let Some((lock, unlock)) = guard.as_ref() {
            let _ = lock.set_enabled(!locked);
            let _ = unlock.set_enabled(locked);
        }
    }
}

fn open_web_url(state: &AppState) -> Result<String, String> {
    let url = state.web_url().ok_or_else(|| "服务尚未启动".to_string())?;
    open::that(&url).map_err(|e| format!("无法打开浏览器：{e}"))?;
    Ok(url)
}

fn launch_web(state: SharedState) {
    tauri::async_runtime::spawn(async move {
        if !state.running.load(Ordering::Relaxed) {
            if let Err(err) = state.start_server().await {
                show_error(&err);
                return;
            }
        }
        if let Err(err) = open_web_url(&state) {
            show_error(&err);
        }
    });
}

pub fn setup(app: &AppHandle, state: SharedState) -> Result<(), String> {
    let show = MenuItem::with_id(app, "show", "显示管理窗口", true, None::<&str>)
        .map_err(|e| format!("无法创建托盘菜单：{e}"))?;
    let web = MenuItem::with_id(app, "web", "打开 Web 文件页", true, None::<&str>)
        .map_err(|e| format!("无法创建托盘菜单：{e}"))?;
    let locked = state.is_locked();
    let lock = MenuItem::with_id(app, "lock", "锁定服务", !locked, None::<&str>)
        .map_err(|e| format!("无法创建托盘菜单：{e}"))?;
    let unlock = MenuItem::with_id(app, "unlock", "解锁服务", locked, None::<&str>)
        .map_err(|e| format!("无法创建托盘菜单：{e}"))?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
        .map_err(|e| format!("无法创建托盘菜单：{e}"))?;
    let menu = MenuBuilder::new(app)
        .item(&show)
        .item(&web)
        .separator()
        .item(&lock)
        .item(&unlock)
        .separator()
        .item(&quit)
        .build()
        .map_err(|e| format!("无法创建托盘菜单：{e}"))?;
    let icon = Image::from_bytes(include_bytes!("../icons/tray.png"))
        .map_err(|e| format!("无法加载托盘图标：{e}"))?;
    let menu_state = state.clone();
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("NB 文件管理器")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "web" => launch_web(menu_state.clone()),
            "lock" => menu_state.set_locked(true),
            "unlock" => menu_state.set_locked(false),
            "quit" => {
                EXITING.store(true, Ordering::SeqCst);
                menu_state.shutdown_now();
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick { .. } => show_main(tray.app_handle()),
            _ => {}
        })
        .build(app)
        .map_err(|e| format!("无法创建系统托盘：{e}"))?;
    if let Ok(mut guard) = LOCK_ITEMS.lock() {
        *guard = Some((lock, unlock));
    }
    state.set_lock_callback(Arc::new(|locked: bool| sync_lock(locked)));
    TRAY_READY.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn get_snapshot(state: State<'_, SharedState>) -> Snapshot {
    state.snapshot()
}

#[tauri::command]
pub async fn start_server(state: State<'_, SharedState>) -> Result<Snapshot, String> {
    let state = state.inner().clone();
    state.start_server().await?;
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn stop_server(state: State<'_, SharedState>) -> Result<Snapshot, String> {
    let state = state.inner().clone();
    state.stop_server().await;
    Ok(state.snapshot())
}

#[tauri::command]
pub fn set_locked(locked: bool, state: State<'_, SharedState>) -> Snapshot {
    state.set_locked(locked);
    state.snapshot()
}

#[tauri::command]
pub async fn reset_password(
    new_password: String,
    state: State<'_, SharedState>,
) -> Result<Snapshot, String> {
    let state = state.inner().clone();
    let mut cfg = state.cfg();
    if new_password.is_empty() {
        cfg.password_hash = None;
        state.log.line("桌面端已清除网页访问密码");
    } else {
        crate::auth::validate_password(&new_password)?;
        let password = new_password;
        let hash =
            tauri::async_runtime::spawn_blocking(move || crate::auth::hash_password(&password))
                .await
                .map_err(|e| format!("密码处理失败：{e}"))?
                .map_err(|e| e)?;
        cfg.password_hash = Some(hash);
        state.log.line("桌面端已重置网页访问密码");
    }
    cfg.rotate_auth_epoch();
    state.save_config(cfg)?;
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn pick_directory(current: Option<String>) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = current.as_ref() {
            if !dir.trim().is_empty() {
                dialog = dialog.set_directory(dir);
            }
        }
        dialog
            .pick_folder()
            .map(|path| path.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| format!("无法打开文件夹选择框：{e}"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPayload {
    pub port: u16,
    pub bind_lan: bool,
    pub root_dir: String,
    pub upload_limit_mb: u64,
    pub conflict_mode: ConflictMode,
    pub zip_compress: ZipCompress,
    pub zip_include_folder: bool,
    pub recycle_days: u32,
    pub session_hours: u32,
    pub max_login_failures: u32,
    pub login_block_minutes: u32,
    pub auto_start: bool,
}

#[derive(Serialize)]
pub struct SaveSettingsResult {
    pub snapshot: Snapshot,
    pub restarting: bool,
    pub message: String,
}

#[tauri::command]
pub fn save_settings(
    settings: SettingsPayload,
    state: State<'_, SharedState>,
) -> Result<SaveSettingsResult, String> {
    let state = state.inner().clone();
    let current = state.cfg();
    if settings.root_dir.trim().is_empty() {
        return Err("共享目录不能为空".into());
    }
    let running = state.running.load(Ordering::Relaxed);
    let old_port = {
        let port = state.bound_port.load(Ordering::Relaxed);
        if port == 0 {
            current.port
        } else {
            port
        }
    };
    let old_bind = state.bound_bind_lan.load(Ordering::Relaxed);
    let mut next = current.clone();
    next.port = settings.port;
    next.bind_lan = settings.bind_lan;
    next.root_dir = settings.root_dir;
    next.upload_limit_mb = settings.upload_limit_mb;
    next.conflict_mode = settings.conflict_mode;
    next.zip_compress = settings.zip_compress;
    next.zip_include_folder = settings.zip_include_folder;
    next.recycle_days = settings.recycle_days;
    next.session_hours = settings.session_hours;
    next.max_login_failures = settings.max_login_failures;
    next.login_block_minutes = settings.login_block_minutes;
    next.auto_start = settings.auto_start;
    state.save_config(next.clone())?;
    let restarting = running && (next.port != current.port || next.bind_lan != current.bind_lan);
    if restarting {
        let state2 = state.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = state2.restart_server(old_port, old_bind).await;
        });
    }
    let message = if restarting {
        "设置已保存，正在重启服务".to_string()
    } else {
        "设置已保存并生效".to_string()
    };
    Ok(SaveSettingsResult {
        snapshot: state.snapshot(),
        restarting,
        message,
    })
}

#[tauri::command]
pub fn get_logs(
    kind: String,
    page: Option<usize>,
    page_size: Option<usize>,
    state: State<'_, SharedState>,
) -> Result<serde_json::Value, String> {
    let page = page.unwrap_or(1).max(1);
    let page_size = page_size.unwrap_or(50).clamp(10, 500);
    let value = match kind.as_str() {
        "upload" => serde_json::to_value(state.log.audit_page("upload", page, page_size)),
        "download" => serde_json::to_value(state.log.audit_page("download", page, page_size)),
        _ => serde_json::to_value(state.log.access_page(page, page_size)),
    }
    .map_err(|e| format!("序列化日志失败：{e}"))?;
    Ok(value)
}

#[tauri::command]
pub fn clear_logs(state: State<'_, SharedState>) {
    state.log.clear_all();
}

#[tauri::command]
pub async fn get_recycle(state: State<'_, SharedState>) -> Result<serde_json::Value, String> {
    let state2 = state.inner().clone();
    let days = state2.cfg().recycle_days;
    let handle = tauri::async_runtime::spawn_blocking(move || crate::recycle::scan(&state2));
    let entries = handle.await.map_err(|e| format!("读取回收站失败：{e}"))?;
    Ok(serde_json::json!({ "entries": entries, "days": days }))
}

#[tauri::command]
pub async fn restore_recycle(
    names: Vec<String>,
    state: State<'_, SharedState>,
) -> Result<serde_json::Value, String> {
    let state2 = state.inner().clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || crate::recycle::restore(&state2, &names))
            .await
            .map_err(|e| format!("还原失败：{e}"))?;
    Ok(serde_json::json!({ "ok": result.0, "failures": result.1 }))
}

#[tauri::command]
pub async fn purge_recycle(
    names: Vec<String>,
    state: State<'_, SharedState>,
) -> Result<serde_json::Value, String> {
    let state2 = state.inner().clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || crate::recycle::purge(&state2, &names))
            .await
            .map_err(|e| format!("永久删除失败：{e}"))?;
    Ok(serde_json::json!({ "ok": result.0, "failures": result.1 }))
}

#[tauri::command]
pub async fn clear_recycle(state: State<'_, SharedState>) -> Result<serde_json::Value, String> {
    let state2 = state.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || crate::recycle::clear(&state2))
        .await
        .map_err(|e| format!("清空回收站失败：{e}"))?;
    Ok(serde_json::json!({ "ok": result.0, "failures": result.1 }))
}

#[tauri::command]
pub fn generate_nginx(backend: String, prefix: String, listen: String) -> String {
    crate::nginx::build_nginx_config(&backend, &prefix, &listen)
}

#[tauri::command]
pub async fn open_web(state: State<'_, SharedState>) -> Result<String, String> {
    let state = state.inner().clone();
    if !state.running.load(Ordering::Relaxed) {
        state.start_server().await?;
    }
    open_web_url(&state)
}

#[tauri::command]
pub fn open_shared_dir(state: State<'_, SharedState>) -> Result<(), String> {
    let dir = state.cfg().root_dir;
    open::that(&dir).map_err(|e| format!("无法打开共享目录：{e}"))
}

#[tauri::command]
pub fn open_data_dir(state: State<'_, SharedState>) -> Result<(), String> {
    open::that(&state.data_dir).map_err(|e| format!("无法打开数据目录：{e}"))
}

#[tauri::command]
pub fn open_logs_dir(state: State<'_, SharedState>) -> Result<(), String> {
    let dir = state.data_dir.join("logs");
    open::that(&dir).map_err(|e| format!("无法打开日志目录：{e}"))
}

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("只允许打开 HTTP 地址".into());
    }
    open::that(&url).map_err(|e| format!("无法打开浏览器：{e}"))
}

pub fn run() {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    let data_dir = config::resolve_data_dir(exe_dir);
    if let Err(err) = std::fs::create_dir_all(&data_dir) {
        show_error(&format!("无法创建数据目录 {}：{err}", data_dir.display()));
        return;
    }
    let instance_lock = match crate::state::acquire_instance(&data_dir) {
        Ok(file) => file,
        Err(err) => {
            show_error(&err);
            return;
        }
    };
    let (cfg, warning) = config::load(&data_dir);
    let log = crate::logging::LogStore::new(&data_dir);
    if let Some(warning) = warning {
        log.line(&warning);
    }
    // 首次启动即落盘，确保会话密钥与默认共享目录跨重启保持稳定
    if let Err(err) = config::save(&data_dir, &cfg) {
        log.line(&format!("初始化配置失败：{err}"));
    }
    let state = AppState::new(data_dir, cfg, log, Some(instance_lock));
    let builder = tauri::Builder::default()
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            start_server,
            stop_server,
            set_locked,
            reset_password,
            save_settings,
            get_logs,
            clear_logs,
            get_recycle,
            restore_recycle,
            purge_recycle,
            clear_recycle,
            generate_nginx,
            pick_directory,
            open_web,
            open_shared_dir,
            open_data_dir,
            open_logs_dir,
            open_url
        ])
        .setup(move |app| {
            let state = app.state::<SharedState>().inner().clone();
            if let Err(err) = setup(app.handle(), state.clone()) {
                state.log.line(&format!("系统托盘初始化失败：{err}"));
            }
            if state.cfg().auto_start {
                tauri::async_runtime::spawn(async move {
                    let _ = state.start_server().await;
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if tray_ready() && !is_exiting() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        });
    if let Err(err) = builder.run(tauri::generate_context!()) {
        show_error(&format!("启动界面初始化失败：{err}"));
    }
}
