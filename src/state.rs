use crate::auth::LoginLimiter;
use crate::config::{self, AppConfig};
use crate::logging::LogStore;
use crate::paths::DirSizeCache;
use crate::tasks::TaskManager;
use crate::uploads::UploadManager;
use fs2::FileExt;
use serde::Serialize;
use std::fs::OpenOptions;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

pub struct Stats {
    started: Mutex<Option<Instant>>,
    upload_bytes: AtomicU64,
    download_bytes: AtomicU64,
    active: AtomicI64,
    clients: Mutex<std::collections::HashMap<String, Instant>>,
}

#[derive(Serialize)]
pub struct StatsView {
    pub uptime_secs: u64,
    pub upload_bytes: u64,
    pub download_bytes: u64,
    pub online_clients: usize,
    pub active_requests: i64,
}

impl Stats {
    fn new() -> Self {
        Self {
            started: Mutex::new(None),
            upload_bytes: AtomicU64::new(0),
            download_bytes: AtomicU64::new(0),
            active: AtomicI64::new(0),
            clients: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn start(&self) {
        if let Ok(mut guard) = self.started.lock() {
            *guard = Some(Instant::now());
        }
        self.upload_bytes.store(0, Ordering::Relaxed);
        self.download_bytes.store(0, Ordering::Relaxed);
        self.active.store(0, Ordering::Relaxed);
        if let Ok(mut guard) = self.clients.lock() {
            guard.clear();
        }
    }

    pub fn stop(&self) {
        if let Ok(mut guard) = self.started.lock() {
            *guard = None;
        }
        self.active.store(0, Ordering::Relaxed);
        if let Ok(mut guard) = self.clients.lock() {
            guard.clear();
        }
    }

    pub fn add_upload(&self, n: u64) {
        self.upload_bytes.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_download(&self, n: u64) {
        self.download_bytes.fetch_add(n, Ordering::Relaxed);
    }

    pub fn request_start(&self) {
        self.active.fetch_add(1, Ordering::Relaxed);
    }

    pub fn request_end(&self) {
        let _ = self
            .active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1).max(0))
            });
    }

    pub fn touch_client(&self, ip: &str) {
        let now = Instant::now();
        if let Ok(mut guard) = self.clients.lock() {
            if guard.len() > 5000 {
                guard.clear();
            }
            guard.insert(ip.to_string(), now);
        }
    }

    pub fn online_clients(&self) -> usize {
        let now = Instant::now();
        let Ok(mut guard) = self.clients.lock() else {
            return 0;
        };
        guard.retain(|_, t| now.duration_since(*t) < Duration::from_secs(30));
        guard.len()
    }

    pub fn view(&self) -> StatsView {
        let uptime = self
            .started
            .lock()
            .ok()
            .and_then(|g| *g)
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);
        StatsView {
            uptime_secs: uptime,
            upload_bytes: self.upload_bytes.load(Ordering::Relaxed),
            download_bytes: self.download_bytes.load(Ordering::Relaxed),
            online_clients: self.online_clients(),
            active_requests: self.active.load(Ordering::Relaxed),
        }
    }
}

pub struct ServerControl {
    pub running: bool,
    pub port: u16,
    pub shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    pub join: Option<tokio::task::JoinHandle<()>>,
}

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub data_dir: PathBuf,
    pub config: RwLock<AppConfig>,
    root_path: RwLock<PathBuf>,
    pub log: LogStore,
    pub dir_sizes: DirSizeCache,
    pub stats: Stats,
    pub locked: AtomicBool,
    pub limiter: LoginLimiter,
    pub tasks: TaskManager,
    pub uploads: UploadManager,
    pub recycle_lock: Mutex<()>,
    pub server: tokio::sync::Mutex<ServerControl>,
    pub running: AtomicBool,
    pub bound_port: AtomicU16,
    pub bound_bind_lan: AtomicBool,
    pub last_server_error: Mutex<Option<String>>,
    lock_callback: Mutex<Option<Arc<dyn Fn(bool) + Send + Sync>>>,
    maintenance_started: AtomicBool,
    _instance_lock: Mutex<Option<std::fs::File>>,
}

#[derive(Serialize)]
pub struct Snapshot {
    pub version: String,
    pub running: bool,
    pub port: u16,
    pub bind_lan: bool,
    pub root_dir: String,
    pub data_dir: String,
    pub has_password: bool,
    pub auto_start: bool,
    pub upload_limit_mb: u64,
    pub conflict_mode: crate::config::ConflictMode,
    pub zip_compress: crate::config::ZipCompress,
    pub zip_include_folder: bool,
    pub recycle_days: u32,
    pub session_hours: u32,
    pub max_login_failures: u32,
    pub login_block_minutes: u32,
    pub locked: bool,
    pub urls: Vec<String>,
    pub stats: StatsView,
    pub disk_total: Option<u64>,
    pub disk_free: Option<u64>,
    pub log_path: String,
    pub last_server_error: Option<String>,
}

pub fn acquire_instance(data_dir: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(data_dir)
        .map_err(|e| format!("无法创建数据目录 {}：{e}", data_dir.display()))?;
    let path = data_dir.join("instance.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| format!("无法创建实例锁：{e}"))?;
    file.try_lock_exclusive()
        .map_err(|_| "程序已在运行中，请勿重复启动。".to_string())?;
    Ok(file)
}

impl AppState {
    pub fn new(
        data_dir: PathBuf,
        cfg: AppConfig,
        log: LogStore,
        instance_lock: Option<std::fs::File>,
    ) -> Arc<Self> {
        let canonical_root =
            std::fs::canonicalize(&cfg.root_dir).unwrap_or_else(|_| PathBuf::from(&cfg.root_dir));
        Arc::new(Self {
            data_dir,
            config: RwLock::new(cfg),
            root_path: RwLock::new(canonical_root),
            log,
            dir_sizes: DirSizeCache::new(),
            stats: Stats::new(),
            locked: AtomicBool::new(false),
            limiter: LoginLimiter::new(),
            tasks: TaskManager::new(),
            uploads: UploadManager::new(),
            recycle_lock: Mutex::new(()),
            server: tokio::sync::Mutex::new(ServerControl {
                running: false,
                port: 0,
                shutdown: None,
                join: None,
            }),
            running: AtomicBool::new(false),
            bound_port: AtomicU16::new(0),
            bound_bind_lan: AtomicBool::new(true),
            last_server_error: Mutex::new(None),
            lock_callback: Mutex::new(None),
            maintenance_started: AtomicBool::new(false),
            _instance_lock: Mutex::new(instance_lock),
        })
    }

    pub fn cfg(&self) -> AppConfig {
        self.config
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn root(&self) -> PathBuf {
        self.root_path
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    fn refresh_root(&self, cfg: &AppConfig) {
        let canonical =
            std::fs::canonicalize(&cfg.root_dir).unwrap_or_else(|_| PathBuf::from(&cfg.root_dir));
        if let Ok(mut guard) = self.root_path.write() {
            *guard = canonical;
        }
    }

    pub fn save_config(&self, mut cfg: AppConfig) -> Result<(), String> {
        config::validate(&mut cfg, &self.data_dir)?;
        config::save(&self.data_dir, &cfg)?;
        let old_root = self.cfg().root_dir;
        if let Ok(mut guard) = self.config.write() {
            *guard = cfg.clone();
        }
        self.refresh_root(&cfg);
        if old_root != cfg.root_dir {
            self.dir_sizes.invalidate();
        }
        Ok(())
    }

    pub fn replace_config(&self, cfg: AppConfig) {
        if let Ok(mut guard) = self.config.write() {
            *guard = cfg.clone();
        }
        self.refresh_root(&cfg);
        self.dir_sizes.invalidate();
    }

    pub fn set_locked(&self, locked: bool) {
        let previous = self.locked.swap(locked, Ordering::SeqCst);
        if previous != locked {
            self.log.line(if locked {
                "服务已锁定，所有网页访问将被拒绝"
            } else {
                "服务已解锁，恢复网页访问"
            });
            let callback = self
                .lock_callback
                .lock()
                .ok()
                .and_then(|g| g.as_ref().cloned());
            if let Some(cb) = callback {
                cb(locked);
            }
        }
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }

    pub fn set_lock_callback(&self, callback: Arc<dyn Fn(bool) + Send + Sync>) {
        if let Ok(mut guard) = self.lock_callback.lock() {
            *guard = Some(callback);
        }
    }

    pub fn set_last_error(&self, message: Option<String>) {
        if let Ok(mut guard) = self.last_server_error.lock() {
            *guard = message;
        }
    }

    pub fn local_urls(&self) -> Vec<String> {
        if !self.running.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let cfg = self.cfg();
        let port = self.bound_port.load(Ordering::Relaxed);
        if port == 0 {
            return Vec::new();
        }
        let mut ips: Vec<String> = vec!["127.0.0.1".to_string()];
        if cfg.bind_lan {
            if let Ok(list) = local_ip_address::list_afinet_netifas() {
                for (_name, ip) in list {
                    if let IpAddr::V4(v4) = ip {
                        if !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified() {
                            ips.push(v4.to_string());
                        }
                    }
                }
            }
        }
        ips.sort();
        ips.dedup();
        ips.into_iter()
            .map(|ip| format!("http://{ip}:{port}"))
            .collect()
    }

    pub fn snapshot(&self) -> Snapshot {
        let cfg = self.cfg();
        let running = self.running.load(Ordering::Relaxed);
        let port = if running {
            self.bound_port.load(Ordering::Relaxed)
        } else {
            cfg.port
        };
        let root = self.root();
        let disk_total = fs2::total_space(&root).ok();
        let disk_free = fs2::available_space(&root).ok();
        Snapshot {
            version: config::APP_VERSION.to_string(),
            running,
            port,
            bind_lan: cfg.bind_lan,
            root_dir: cfg.root_dir.clone(),
            data_dir: self.data_dir.to_string_lossy().to_string(),
            has_password: cfg.has_password(),
            auto_start: cfg.auto_start,
            upload_limit_mb: cfg.upload_limit_mb,
            conflict_mode: cfg.conflict_mode,
            zip_compress: cfg.zip_compress,
            zip_include_folder: cfg.zip_include_folder,
            recycle_days: cfg.recycle_days,
            session_hours: cfg.session_hours,
            max_login_failures: cfg.max_login_failures,
            login_block_minutes: cfg.login_block_minutes,
            locked: self.is_locked(),
            urls: self.local_urls(),
            stats: self.stats.view(),
            disk_total,
            disk_free,
            log_path: self.log.file_path().to_string_lossy().to_string(),
            last_server_error: self.last_server_error.lock().ok().and_then(|g| g.clone()),
        }
    }

    pub fn web_url(&self) -> Option<String> {
        if !self.running.load(Ordering::Relaxed) {
            return None;
        }
        let port = self.bound_port.load(Ordering::Relaxed);
        if port == 0 {
            return None;
        }
        Some(format!("http://127.0.0.1:{port}/"))
    }

    pub async fn start_server(self: &Arc<Self>) -> Result<u16, String> {
        let mut guard = self.server.lock().await;
        if guard.running {
            return Ok(guard.port);
        }
        let cfg = self.cfg();
        let bind_ip = if cfg.bind_lan {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        };
        let addr = SocketAddr::new(bind_ip, cfg.port);
        let std_listener = match std::net::TcpListener::bind(addr) {
            Ok(listener) => listener,
            Err(err) => {
                let message = format!("端口 {} 启动失败（可能已被占用）：{err}", cfg.port);
                self.set_last_error(Some(message.clone()));
                self.log.line(&message);
                return Err(message);
            }
        };
        if let Err(err) = std_listener.set_nonblocking(true) {
            let message = format!("无法配置监听端口：{err}");
            self.set_last_error(Some(message.clone()));
            return Err(message);
        }
        let listener = match tokio::net::TcpListener::from_std(std_listener) {
            Ok(listener) => listener,
            Err(err) => {
                let message = format!("无法创建监听器：{err}");
                self.set_last_error(Some(message.clone()));
                return Err(message);
            }
        };
        let port = match listener.local_addr() {
            Ok(addr) => addr.port(),
            Err(err) => {
                let message = format!("无法读取监听地址：{err}");
                self.set_last_error(Some(message.clone()));
                return Err(message);
            }
        };
        let router = crate::server::build_router(self.clone());
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let join = tokio::spawn(async move {
            let serve = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            });
            if let Err(err) = serve.await {
                eprintln!("HTTP 服务异常退出：{err}");
            }
        });
        guard.running = true;
        guard.port = port;
        guard.shutdown = Some(tx);
        guard.join = Some(join);
        drop(guard);
        self.running.store(true, Ordering::SeqCst);
        self.bound_port.store(port, Ordering::SeqCst);
        self.bound_bind_lan.store(cfg.bind_lan, Ordering::SeqCst);
        self.set_last_error(None);
        self.stats.start();
        self.log.line(&format!(
            "Web 服务已启动：端口 {port}，监听 {}，共享目录 {}",
            if cfg.bind_lan { "0.0.0.0" } else { "127.0.0.1" },
            cfg.root_dir
        ));
        self.spawn_maintenance();
        Ok(port)
    }

    pub async fn stop_server(&self) {
        let (tx, mut join, was_running) = {
            let mut guard = self.server.lock().await;
            if !guard.running {
                (None, None, false)
            } else {
                guard.running = false;
                (guard.shutdown.take(), guard.join.take(), true)
            }
        };
        if !was_running {
            return;
        }
        if let Some(tx) = tx {
            let _ = tx.send(());
        }
        if let Some(handle) = join.as_mut() {
            if tokio::time::timeout(Duration::from_secs(5), &mut *handle)
                .await
                .is_err()
            {
                handle.abort();
                let _ = handle.await;
            }
        }
        self.running.store(false, Ordering::SeqCst);
        self.bound_port.store(0, Ordering::SeqCst);
        self.stats.stop();
        self.log.line("Web 服务已停止");
    }

    pub fn shutdown_now(&self) {
        if let Ok(mut guard) = self.server.try_lock() {
            if let Some(tx) = guard.shutdown.take() {
                let _ = tx.send(());
            }
            guard.running = false;
        }
        self.running.store(false, Ordering::SeqCst);
    }

    pub async fn restart_server(
        self: &Arc<Self>,
        old_port: u16,
        old_bind: bool,
    ) -> Result<u16, String> {
        self.stop_server().await;
        match self.start_server().await {
            Ok(port) => Ok(port),
            Err(err) => {
                let mut cfg = self.cfg();
                cfg.port = old_port;
                cfg.bind_lan = old_bind;
                let _ = config::save(&self.data_dir, &cfg);
                self.replace_config(cfg);
                let message = format!("{err}；已恢复原端口配置");
                self.set_last_error(Some(message.clone()));
                self.log.line(&message);
                let _ = self.start_server().await;
                Err(message)
            }
        }
    }

    fn spawn_maintenance(self: &Arc<Self>) {
        if self.maintenance_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let state = self.clone();
        tokio::spawn(async move {
            let initial = state.clone();
            let _ = tokio::task::spawn_blocking(move || initial.startup_cleanup()).await;
            loop {
                tokio::time::sleep(Duration::from_secs(300)).await;
                let state = state.clone();
                let _ = tokio::task::spawn_blocking(move || state.sweep_once()).await;
            }
        });
    }

    fn tmp_dir(&self) -> PathBuf {
        self.data_dir.join(config::TMP_DIR)
    }

    fn startup_cleanup(&self) {
        let removed_tasks = self.tasks.remove_finished();
        let purged_recycle = crate::recycle::purge_expired(self);
        let purged_staging = crate::uploads::purge_staging(&self.root());
        let mut removed_tmp = 0;
        if let Ok(entries) = std::fs::read_dir(self.tmp_dir()) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                    && std::fs::remove_file(entry.path()).is_ok()
                {
                    removed_tmp += 1;
                }
            }
        }
        if removed_tasks as u64 + purged_recycle + purged_staging + removed_tmp > 0 {
            self.log.line(&format!(
                "启动清理：临时任务 {removed_tasks}，回收站超期 {purged_recycle}，上传分片 {purged_staging}，临时文件 {removed_tmp}"
            ));
        }
    }

    fn sweep_once(&self) {
        let tasks = self.tasks.sweep(Duration::from_secs(1800));
        let recycle = crate::recycle::purge_expired(self);
        let staging = crate::uploads::purge_staging(&self.root());
        let mut tmp = 0;
        if let Ok(entries) = std::fs::read_dir(self.tmp_dir()) {
            let now = std::time::SystemTime::now();
            for entry in entries.flatten() {
                let old = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| now.duration_since(t).ok())
                    .map(|age| age > Duration::from_secs(24 * 3600))
                    .unwrap_or(false);
                if old
                    && entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                    && std::fs::remove_file(entry.path()).is_ok()
                {
                    tmp += 1;
                }
            }
        }
        if tasks as u64 + recycle + staging + tmp > 0 {
            self.log.line(&format!(
                "定期清理：过期任务 {tasks}，回收站 {recycle}，上传分片 {staging}，临时文件 {tmp}"
            ));
        }
    }

    pub fn temp_dir(&self) -> PathBuf {
        let dir = self.tmp_dir();
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}
