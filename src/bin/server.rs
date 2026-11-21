use nb_file_manager::auth;
use nb_file_manager::config::{self, APP_NAME};
use nb_file_manager::logging::LogStore;
use nb_file_manager::state::{self, AppState};
use std::path::PathBuf;

#[derive(Default)]
struct Cli {
    port: Option<u16>,
    dir: Option<String>,
    data_dir: Option<PathBuf>,
    password: Option<Option<String>>,
    max_upload_mb: Option<u64>,
    bind: Option<bool>,
    no_banner: bool,
    help: bool,
    version: bool,
}

fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut index = 0;
    while index < args.len() {
        let raw = &args[index];
        let (key, inline_value) = match raw.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (raw.as_str(), None),
        };
        let mut take_value = |label: &str| -> Result<String, String> {
            if let Some(value) = &inline_value {
                return Ok(value.clone());
            }
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| format!("参数 {label} 缺少取值"))
        };
        match key {
            "-h" | "--help" => cli.help = true,
            "--version" => cli.version = true,
            "--no-banner" => cli.no_banner = true,
            "--no-password" => cli.password = Some(None),
            "--port" => {
                let value = take_value("--port")?;
                cli.port = Some(
                    value
                        .parse()
                        .map_err(|_| format!("端口格式不正确：{value}"))?,
                );
            }
            "--dir" => cli.dir = Some(take_value("--dir")?),
            "--data-dir" => cli.data_dir = Some(PathBuf::from(take_value("--data-dir")?)),
            "--password" => cli.password = Some(Some(take_value("--password")?)),
            "--max-upload" => {
                let value = take_value("--max-upload")?;
                cli.max_upload_mb = Some(
                    value
                        .parse()
                        .map_err(|_| format!("上传上限格式不正确：{value}"))?,
                );
            }
            "--bind" => {
                let value = take_value("--bind")?;
                cli.bind = Some(match value.to_ascii_lowercase().as_str() {
                    "lan" => true,
                    "loopback" | "local" => false,
                    other => return Err(format!("未知的监听范围：{other}")),
                });
            }
            other => return Err(format!("未知参数：{other}")),
        }
        index += 1;
    }
    Ok(cli)
}

fn help_text() -> String {
    format!(
        "{APP_NAME} 命令行服务\n\n\
用法：nb-file-server [选项]\n\n\
选项：\n\
  --port <端口>          监听端口（默认读取保存的配置，首次为 8888）\n\
  --dir <目录>           共享目录（默认 数据目录/files）\n\
  --data-dir <目录>      配置、日志与临时数据目录\n\
  --password <密码>      设置网页访问密码\n\
  --no-password          清除访问密码（局域网内可匿名访问，慎用）\n\
  --max-upload <MB>      单文件上传上限，0 表示无限制\n\
  --bind <lan|loopback>  监听 0.0.0.0 或 127.0.0.1\n\
  --no-banner            不打印启动横幅\n\
  -h, --help             显示帮助\n\
  --version              显示版本\n"
    )
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse_args(&args) {
        Ok(cli) => cli,
        Err(err) => {
            eprintln!("参数错误：{err}\n\n{}", help_text());
            std::process::exit(2);
        }
    };
    if cli.help {
        println!("{}", help_text());
        return;
    }
    if cli.version {
        println!("{APP_NAME} v{}", config::APP_VERSION);
        return;
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    let data_dir = cli
        .data_dir
        .clone()
        .unwrap_or_else(|| config::resolve_data_dir(exe_dir));
    if let Err(err) = std::fs::create_dir_all(&data_dir) {
        eprintln!("无法创建数据目录 {}：{err}", data_dir.display());
        std::process::exit(1);
    }
    let instance_lock = match state::acquire_instance(&data_dir) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };
    let (mut cfg, warning) = config::load(&data_dir);
    let mut printed_password: Option<String> = None;
    if let Some(port) = cli.port {
        cfg.port = port;
    }
    if let Some(dir) = cli.dir {
        cfg.root_dir = dir;
    }
    if let Some(bind) = cli.bind {
        cfg.bind_lan = bind;
    }
    if let Some(limit) = cli.max_upload_mb {
        cfg.upload_limit_mb = limit;
    }
    match cli.password {
        Some(Some(password)) => {
            if let Err(err) = auth::validate_password(&password) {
                eprintln!("{err}");
                std::process::exit(2);
            }
            match auth::hash_password(&password) {
                Ok(hash) => {
                    cfg.password_hash = Some(hash);
                    cfg.rotate_auth_epoch();
                    printed_password = Some(password);
                }
                Err(err) => {
                    eprintln!("{err}");
                    std::process::exit(1);
                }
            }
        }
        Some(None) => {
            cfg.password_hash = None;
            cfg.rotate_auth_epoch();
        }
        None => {}
    }
    if let Err(err) = config::validate(&mut cfg, &data_dir) {
        eprintln!("配置无效：{err}");
        std::process::exit(1);
    }
    if let Err(err) = config::save(&data_dir, &cfg) {
        eprintln!("{err}");
        std::process::exit(1);
    }
    let log = LogStore::new(&data_dir);
    if let Some(warning) = warning {
        log.line(&warning);
        eprintln!("警告：{warning}");
    }
    let state = AppState::new(data_dir.clone(), cfg.clone(), log, Some(instance_lock));
    let port = match state.start_server().await {
        Ok(port) => port,
        Err(err) => {
            eprintln!("启动失败：{err}");
            std::process::exit(1);
        }
    };
    if !cli.no_banner {
        let line = "=".repeat(64);
        println!("\n{line}");
        println!(" {APP_NAME} v{} · 命令行模式", config::APP_VERSION);
        println!("{}", "-".repeat(64));
        match &printed_password {
            Some(pw) => println!(" 访问密码：{pw}"),
            None if cfg.has_password() => println!(" 访问密码：已设置"),
            None => println!(" 访问密码：未设置（局域网内任何人可访问，建议设置）"),
        }
        println!(
            " 监听地址：{}（{}）",
            if cfg.bind_lan { "0.0.0.0" } else { "127.0.0.1" },
            if cfg.bind_lan {
                "允许局域网"
            } else {
                "仅本机"
            }
        );
        println!(" 监听端口：{port}");
        println!(" 共享目录：{}", cfg.root_dir);
        println!(" 数据目录：{}", data_dir.display());
        println!(" 配置文件：{}", config::config_path(&data_dir).display());
        println!(" 日志文件：{}", state.log.file_path().display());
        println!(" 访问地址：");
        for url in state.local_urls() {
            println!("   {url}");
        }
        println!("{}", "-".repeat(64));
        println!(" 按 Ctrl+C 停止服务");
        println!("{line}\n");
    }
    if let Err(err) = tokio::signal::ctrl_c().await {
        eprintln!("等待退出信号失败：{err}");
    }
    state.stop_server().await;
    println!("服务已停止。");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parsing() {
        let args = vec![
            "--port".to_string(),
            "9999".to_string(),
            "--dir=D:\\data".to_string(),
            "--no-password".to_string(),
            "--bind".to_string(),
            "loopback".to_string(),
        ];
        let cli = parse_args(&args).unwrap();
        assert_eq!(cli.port, Some(9999));
        assert_eq!(cli.dir.as_deref(), Some("D:\\data"));
        assert_eq!(cli.password, Some(None));
        assert_eq!(cli.bind, Some(false));
        assert!(parse_args(&["--port".into()]).is_err());
        assert!(parse_args(&["--unknown".into()]).is_err());
    }
}
