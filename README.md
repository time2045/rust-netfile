# NB 文件管理器

[![Build & Release](https://github.com/time2045/rust-netfile/actions/workflows/build-windows.yml/badge.svg)](https://github.com/time2045/rust-netfile/actions/workflows/build-windows.yml)

当前版本：v1.0.0 · [下载最新版](https://github.com/time2045/rust-netfile/releases/latest)

面向局域网的文件共享与轻量文件服务器：Windows 桌面端（Tauri 启动台 + 系统托盘）与 Linux 无界面服务共用同一套 Rust 内核和 Web 界面。单可执行文件、无须数据库、无须 Nginx 即可运行。

本项目是对原 Python 版 `file_server`（Flask + waitress + Tkinter）的完整 Rust 重写，并在此基础上补齐了分片续传、Web 控制台、预览、网格视图、回收站管理、任务进度、Argon2 密码哈希、CSRF、按 IP 登录限流等能力。

## 主要功能

- **文件管理**：目录浏览、递归搜索、按名称/大小/时间排序、分页、列表/网格视图、图片缩略图。
- **在线预览**：图片、视频、音频直接预览；文本/代码/日志/JSON 等文本文件支持在线查看（最多 512 KB）。
- **上传**：8 MB 分片 + 断点续传，单文件自动重试，多文件并发 3 个；支持拖拽文件夹、粘贴截图上传；同名冲突支持询问/自动重命名/覆盖。
- **下载**：单文件 HTTP Range 断点下载；文件夹与多选项目后台 ZIP 打包，实时进度，支持压缩级别与“包含顶层文件夹”。
- **文件操作**：新建、重命名、移动、复制、批量移动/复制/删除；删除进入回收站，保留期可配置，超期自动清理。
- **Web 控制台**（登录后可用）：运行概览、端口/监听范围/共享目录/上传限制/压缩/回收站保留期等配置、日志查看与清空、回收站还原/永久删除/清空、Nginx 配置生成。
- **安全**：Argon2id 密码哈希、HMAC 会话 Cookie、CSRF 双提交校验、按 IP 登录失败限流、服务一键锁定、严格 CSP、路径穿越与符号链接逃逸防护。
- **桌面启动台**：启动/停止服务、访问地址复制、流量与在线设备统计、磁盘空间、密码重置、锁定/解锁；关闭窗口驻留托盘；单实例运行。
- **跨平台**：Windows 桌面单文件 EXE；Linux 无界面服务单文件；Web 界面自适应桌面与手机。

## 与 Python 版的改进

| 方面 | Python 版 | Rust 版 |
| --- | --- | --- |
| 运行时 | Python + PyInstaller，启动慢 | 原生单文件，启动快、内存低 |
| Web 框架 | Flask + waitress 线程池 | axum + tokio 异步 IO |
| 密码 | 明文保存在配置文件 | Argon2id 哈希，不可逆 |
| 登录保护 | 全局 5 次失败锁站 | 按 IP 限流并倒计时，避免恶意锁站 |
| CSRF | 仅 SameSite | Cookie + Header 双提交校验 |
| 搜索/打包 | 同步阻塞，无进度 | 后台任务，实时进度 |
| 上传 | 8 MB 分片 | 分片续传 + 进度 + 自动重试 |
| 管理入口 | Tkinter 桌面窗口 | 登录后的 Web 控制台 + 桌面启动台 |
| 预览 | 无 | 图片/视频/音频/文本在线预览 |
| 查看方式 | 列表 | 列表 + 网格 + 图片缩略图 |

## 下载与运行

### Windows 桌面版

1. 打开 [Releases](https://github.com/time2045/rust-netfile/releases/latest)。
2. 下载 `NB-File-Manager-vX.Y.Z.exe`，双击运行，无需安装。
3. 首次启动会自动启动 Web 服务，并在系统托盘显示图标；窗口内可打开 Web 控制台。

运行要求：

- Windows 10/11 x64。
- 需要 WebView2 Runtime（通常已随 Windows / Edge 安装）。
- 如提示缺少 `VCRUNTIME140.dll` / `VCRUNTIME140_1.dll`，安装 Microsoft Visual C++ 2015-2022 Redistributable（x64）。

> GitHub Releases 会重写中文资源文件名，因此实际下载文件名为 `NB-File-Manager-vX.Y.Z.exe`；资源显示标签为 `NB文件管理器_vX.Y.Z.exe`。

### Linux / 服务器无界面版

下载 `NB-File-Server-vX.Y.Z-linux-x64`：

```bash
chmod +x NB-File-Server-vX.Y.Z-linux-x64
./NB-File-Server-vX.Y.Z-linux-x64 --port 8888 --dir /srv/share --password 'your-password'
```

启动后终端会打印访问地址、共享目录、数据目录与日志路径。浏览器打开地址即可使用；控制台在“控制台”页。

systemd 示例：

```ini
[Unit]
Description=NB File Server
After=network.target

[Service]
ExecStart=/opt/nb/NB-File-Server-v1.0.0-linux-x64 --port 8888 --dir /srv/share
Restart=on-failure
WorkingDirectory=/opt/nb

[Install]
WantedBy=multi-user.target
```

## 首次使用

- 默认端口 `8888`，默认共享目录为“数据目录/files”。
- **默认没有访问密码**。启动台会提示“未设置（可匿名访问）”；请在 Web 控制台的「服务器设置 → 访问密码」或启动台「重置访问密码」中设置密码。
- 数据目录规则：
  1. 环境变量 `NETFILE_DATA_DIR`（桌面与命令行均支持）；
  2. 可执行文件同级的 `data/` 目录（便携优先）；
  3. 不可写时回退到 `%APPDATA%\NBNetFile`（Windows）或 `~/.local/share/nb-file-manager`（Linux）。
- 数据目录内容：
  - `config.json`：服务配置（密码为哈希）；
  - `files/`：默认共享目录；
  - `logs/server.log`：访问与审计日志（自动轮转）；
  - `tmp/`：打包下载的临时 ZIP；
  - `instance.lock`：单实例锁。
- 共享目录下的 `.recycle` 与 `.upload_tmp` 为系统目录，不会在网页中显示，也无法通过 URL 访问。

## 命令行参数（无界面服务）

| 参数 | 说明 |
| --- | --- |
| `--port <端口>` | 监听端口，默认读取配置，首次为 8888 |
| `--dir <目录>` | 共享目录，默认数据目录下的 `files` |
| `--data-dir <目录>` | 配置、日志、临时数据目录 |
| `--password <密码>` | 设置网页访问密码 |
| `--no-password` | 清除访问密码（局域网匿名访问，慎用） |
| `--max-upload <MB>` | 单文件上传限制，0 为无限制 |
| `--bind <lan\|loopback>` | 监听 `0.0.0.0` 或 `127.0.0.1` |
| `--no-banner` | 不打印启动横幅 |
| `-h, --help` / `--version` | 帮助 / 版本 |

## Web 控制台

登录后点击左侧「控制台」：

- **概览**：运行状态、访问地址、运行时长、上下行流量、在线设备、磁盘空间、最近错误。
- **服务器设置**：端口、监听范围、共享目录、上传限制、重名处理、ZIP 压缩、回收站保留天数、登录有效期、登录失败上限与封禁时长、自动启动、访问密码。修改端口或监听范围会自动重启并在完成后自动恢复连接。
- **日志**：访问日志、上传记录、下载记录，分页查看与清空。
- **回收站**：选中还原、永久删除、清空；超期条目由后台任务自动清理。
- **Nginx 配置生成**：填写后端地址、路径前缀与对外端口，一键复制配置。

### Nginx 反向代理

Web 控制台中的生成器会输出类似：

```nginx
server {
    listen 9000;

    location = /file {
        return 301 /file/;
    }

    location /file/ {
        proxy_pass http://127.0.0.1:8888/;
        proxy_set_header X-Forwarded-Prefix /file;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_http_version 1.1;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
        client_max_body_size 0;
        proxy_request_buffering off;
    }
}
```

仅当请求来自本机回环地址时，服务才会信任 `X-Forwarded-Prefix`、`X-Real-IP`、`X-Forwarded-For` 与 `X-Forwarded-Proto`，避免直连客户端伪造。

## 安全说明

- 登录密码以 **Argon2id** 哈希保存，配置文件中没有明文密码；修改密码后全部会话立即失效。
- 会话 Cookie 使用 HMAC-SHA256 签名，`HttpOnly` + `SameSite=Lax`；反向代理 HTTPS 时自动加 `Secure`。
- 所有写操作要求 CSRF Header 与 Cookie 一致；页面启用严格 CSP，无第三方脚本/CDN。
- 同一 IP 连续登录失败达到上限后按配置时长封禁，登录页逐秒倒计时并禁用表单；成功后自动恢复。
- 路径解析在共享根目录内进行 canonical 校验，阻止 `..`、符号链接逃逸与 Windows 设备名/保留字符。
- 预览仅允许图片/视频/音频与纯文本；HTML、SVG 等可能执行脚本的类型强制作为附件下载。
- ZIP 打包跳过符号链接与系统目录；回收站与上传分片对链接条目不做删除/还原。
- **服务锁定**：在 Web 控制台锁定后，所有网页访问返回 423。Windows 端请在启动台或托盘解锁；无界面服务需重启进程。
- 原 Python 版硬编码的桌面启动密码已移除：桌面本机按可信环境处理，网页访问始终由 Argon2 哈希密码保护；请务必设置访问密码。
- 请勿在无密码状态下暴露到不可信网络；本项目不提供免登录分享链接。

## 从源码构建

### 环境

- Rust 1.87+。
- Windows 桌面端需要 MSVC 工具链（Visual Studio 2022 C++ 生成工具 + Windows SDK）。
- Linux 无界面服务只需 `build-essential` 等常规编译环境。

### 命令

```bash
# 命令行服务（Windows / Linux 均可，本机可验证）
cargo run --release --no-default-features --bin nb-file-server -- --port 8888 --dir ./share

# Windows 桌面单文件版
cargo build --release
```

或使用构建脚本（Windows PowerShell）：

```powershell
.\build.ps1 -Mode Test      # 运行测试
.\build.ps1 -Mode Release   # 构建桌面端 + 命令行服务
```

### GitHub Actions 发布

- 推送到 `main` 或提交 PR 时自动编译并运行测试。
- 推送 `v*` 标签自动创建 Release：

```bash
git tag v1.0.0
git push origin v1.0.0
```

构建产物：

- `NB-File-Manager-v1.0.0.exe`：Windows 单文件桌面版（已校验不依赖旁加载 `WebView2Loader.dll`）。
- `NB-File-Server-v1.0.0-linux-x64`：Linux 单文件命令行服务。

## 项目结构

```text
src/
  main.rs            桌面入口（Tauri）
  bin/server.rs      命令行服务入口
  desktop.rs         启动台命令、托盘、单实例
  server/            axum 路由、中间件、上传/下载/控制台/测速 API
  state.rs           运行状态、服务启停、维护任务
  config.rs          配置持久化与校验
  auth.rs            Argon2、会话、CSRF、登录限流
  paths.rs           路径安全、名称校验、目录大小缓存
  archive.rs         ZIP 打包
  fsops.rs           复制/移动
  tasks.rs           后台任务与进度
  recycle.rs         回收站
  uploads.rs         分片上传暂存
  logging.rs         日志环形缓冲与文件轮转
web/                 Web 文件管理器与控制台（登录后使用）
ui/                  桌面启动台（Tauri 前端）
icons/               应用与托盘图标
```

## 常见问题

**端口被占用？**  
控制台修改端口后会自动重启；命令行使用 `--port` 指定其它端口。

**忘记网页密码？**  
Windows 打开启动台「重置访问密码」；Linux 重新以 `--password 新密码` 启动即可覆盖。

**服务被锁定无法登录网页？**  
Windows 在启动台或托盘点击“解锁服务”；无界面服务重启进程即可恢复（锁定状态不持久化）。

**手机能否使用？**  
可以。允许局域网访问时，用手机浏览器打开启动台显示的局域网地址；页面已适配小屏。

**上传几百 MB 的文件安全吗？**  
前端固定 8 MB 分片、失败自动重试并续传；服务端校验偏移与总大小，完成后原子替换。上传期间关闭页面，重新选择同一文件会从已落盘位置继续。
