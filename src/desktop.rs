use crate::config;
use crate::state::{AppState, SharedState, Snapshot};
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

fn open_console_url(state: &AppState) -> Result<String, String> {
    let url = state.web_url().ok_or_else(|| "服务尚未启动".to_string())?;
    open::that(&url).map_err(|e| format!("无法打开浏览器：{e}"))?;
    Ok(url)
}

fn launch_console(state: SharedState) {
    tauri::async_runtime::spawn(async move {
        if !state.running.load(Ordering::Relaxed) {
            if let Err(err) = state.start_server().await {
                show_error(&err);
                return;
            }
        }
        if let Err(err) = open_console_url(&state) {
            show_error(&err);
        }
    });
}

pub fn setup(app: &AppHandle, state: SharedState) -> Result<(), String> {
    let show = MenuItem::with_id(app, "show", "显示启动台", true, None::<&str>)
        .map_err(|e| format!("无法创建托盘菜单：{e}"))?;
    let console = MenuItem::with_id(app, "console", "打开 Web 控制台", true, None::<&str>)
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
        .item(&console)
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
            "console" => launch_console(menu_state.clone()),
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
    state.set_lock_callback(Arc::new(|locked| sync_lock(locked)));
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
pub async fn stop_server(state: State<'_, SharedState>) -> Snapshot {
    let state = state.inner().clone();
    state.stop_server().await;
    state.snapshot()
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

#[tauri::command]
pub async fn open_console(state: State<'_, SharedState>) -> Result<String, String> {
    let state = state.inner().clone();
    if !state.running.load(Ordering::Relaxed) {
        state.start_server().await?;
    }
    open_console_url(&state)
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
    let state = AppState::new(data_dir, cfg, log, Some(instance_lock));
    let builder = tauri::Builder::default()
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            start_server,
            stop_server,
            set_locked,
            reset_password,
            pick_directory,
            open_console,
            open_shared_dir,
            open_data_dir,
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
