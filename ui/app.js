const $ = (sel) => document.querySelector(sel);
const hasTauri = !!(window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke);
const invoke = (cmd, args = {}) => hasTauri
  ? window.__TAURI__.core.invoke(cmd, args)
  : demoInvoke(cmd, args);

let latest = null;
let offline = false;

function formatSize(bytes) {
  const value = Number(bytes);
  if (!Number.isFinite(value) || value < 0) return "—";
  if (value < 1024) return Math.round(value) + " B";
  const units = ["KB", "MB", "GB", "TB"];
  let v = value;
  for (const unit of units) {
    v /= 1024;
    if (v < 1024) return v.toFixed(1) + " " + unit;
  }
  return v.toFixed(1) + " PB";
}

function formatDuration(seconds) {
  const s = Math.max(0, Math.floor(Number(seconds) || 0));
  const pad = (n) => String(n).padStart(2, "0");
  const h = Math.floor(s / 3600);
  if (h >= 24) return Math.floor(h / 24) + " 天 " + (h % 24) + " 小时";
  return pad(h) + ":" + pad(Math.floor((s % 3600) / 60)) + ":" + pad(s % 60);
}

function errText(err) {
  if (typeof err === "string") return err;
  if (err && err.message) return err.message;
  return String(err);
}

function toast(message, type = "error") {
  const box = $("#toastBox");
  const node = document.createElement("div");
  node.className = "toast " + type;
  node.textContent = message;
  box.appendChild(node);
  setTimeout(() => {
    node.style.opacity = "0";
    setTimeout(() => node.remove(), 300);
  }, 3600);
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (err) {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    let ok = false;
    try { ok = document.execCommand("copy"); } catch (e) { ok = false; }
    area.remove();
    return ok;
  }
}

function render(snap) {
  latest = snap;
  $("#version").textContent = "NB 文件管理器 v" + snap.version;
  const locked = snap.locked;
  const running = snap.running;
  const badge = $("#statusBadge");
  badge.className = "badge " + (locked ? "warn" : running ? "ok" : "bad");
  badge.textContent = locked ? "已锁定" : running ? "运行中" : "已停止";
  $("#statusDot").className = "dot " + (locked ? "warn" : running ? "ok" : "bad");
  $("#statusTitle").textContent = locked ? "服务已锁定" : running ? "服务运行中" : "服务已停止";
  $("#statusHint").textContent = running
    ? `端口 ${snap.port} · ${snap.bind_lan ? "允许局域网访问" : "仅本机访问"} · 共享目录 ${snap.root_dir}`
    : `共享目录 ${snap.root_dir}`;
  $("#btnStart").disabled = running;
  $("#btnStop").disabled = !running;
  $("#btnLock").disabled = locked;
  $("#btnUnlock").disabled = !locked;
  $("#btnConsole").disabled = false;

  const urlList = $("#urlList");
  urlList.innerHTML = "";
  if (!snap.urls || !snap.urls.length) {
    const span = document.createElement("span");
    span.className = "muted";
    span.textContent = "服务未运行";
    urlList.appendChild(span);
  } else {
    for (const url of snap.urls) {
      const row = document.createElement("div");
      row.className = "url-row";
      const open = document.createElement("button");
      open.className = "url-link";
      open.textContent = url;
      open.title = "在浏览器中打开";
      open.addEventListener("click", async () => {
        try {
          await invoke("open_url", { url });
        } catch (err) {
          toast(errText(err));
        }
      });
      const copy = document.createElement("button");
      copy.className = "url-copy";
      copy.textContent = "复制";
      copy.addEventListener("click", async () => {
        if (await copyText(url)) toast("地址已复制", "success");
        else toast("复制失败，请手动选择");
      });
      row.append(open, copy);
      urlList.appendChild(row);
    }
  }
  $("#urlHint").textContent = snap.urls && snap.urls.length
    ? "局域网设备使用上述地址访问；如经 Nginx 反代，请使用对外地址。"
    : "启动服务后可在此复制访问地址。";

  $("#statUptime").textContent = running ? formatDuration(snap.stats.uptime_secs) : "—";
  $("#statUpload").textContent = formatSize(snap.stats.upload_bytes || 0);
  $("#statDownload").textContent = formatSize(snap.stats.download_bytes || 0);
  $("#statOnline").textContent = (snap.stats.online_clients || 0) + " 台";

  const total = snap.disk_total || 0;
  const free = snap.disk_free || 0;
  if (total > 0) {
    $("#diskBar").style.width = Math.min(100, ((total - free) / total) * 100).toFixed(1) + "%";
    $("#diskText").textContent = `已用 ${formatSize(total - free)} / 共 ${formatSize(total)} · 剩余 ${formatSize(free)}`;
  } else {
    $("#diskBar").style.width = "0%";
    $("#diskText").textContent = "磁盘信息不可用";
  }
  $("#kvPort").textContent = String(snap.port);
  $("#kvBind").textContent = snap.bind_lan ? "允许局域网" : "仅本机";

  const pwd = $("#pwdState");
  pwd.textContent = snap.has_password ? "已设置" : "未设置（可匿名访问）";
  pwd.className = snap.has_password ? "ok" : "warn";
  $("#autoState").textContent = snap.auto_start ? "已开启" : "已关闭";
  const lockState = $("#lockState");
  lockState.textContent = locked ? "已锁定" : "正常";
  lockState.className = locked ? "warn" : "ok";
  $("#pathRoot").textContent = snap.root_dir;
  $("#pathData").textContent = snap.data_dir;
  $("#pathLog").textContent = snap.log_path;
  const errorCard = $("#errorCard");
  $("#lastError").textContent = snap.last_server_error || "无";
  errorCard.classList.toggle("has-error", !!snap.last_server_error);
  offline = false;
}

async function refresh() {
  try {
    const snap = await invoke("get_snapshot");
    render(snap);
  } catch (err) {
    if (!offline) {
      offline = true;
      toast("读取服务状态失败：" + errText(err));
    }
    $("#statusBadge").className = "badge bad";
    $("#statusBadge").textContent = "无法连接";
  }
}

async function withBusy(button, fn) {
  if (!button) return;
  const text = button.textContent;
  button.disabled = true;
  button.textContent = "处理中…";
  try {
    await fn();
  } catch (err) {
    toast(errText(err));
  } finally {
    button.textContent = text;
    button.disabled = false;
    if (latest) render(latest);
  }
}

function bindActions() {
  $("#btnStart").addEventListener("click", () => withBusy($("#btnStart"), async () => {
    const snap = await invoke("start_server");
    render(snap);
    toast("服务已启动", "success");
  }));
  $("#btnStop").addEventListener("click", () => withBusy($("#btnStop"), async () => {
    const snap = await invoke("stop_server");
    render(snap);
    toast("服务已停止", "info");
  }));
  $("#btnLock").addEventListener("click", () => withBusy($("#btnLock"), async () => {
    if (!confirm("锁定后所有网页访问将被拒绝，需要在启动台或托盘解锁。确定锁定吗？")) return;
    const snap = await invoke("set_locked", { locked: true });
    render(snap);
    toast("服务已锁定", "warning");
  }));
  $("#btnUnlock").addEventListener("click", () => withBusy($("#btnUnlock"), async () => {
    const snap = await invoke("set_locked", { locked: false });
    render(snap);
    toast("服务已解锁", "success");
  }));
  $("#btnConsole").addEventListener("click", () => withBusy($("#btnConsole"), async () => {
    const url = await invoke("open_console");
    toast("已在浏览器打开：" + url, "success");
  }));
  $("#btnOpenDir").addEventListener("click", () => withBusy($("#btnOpenDir"), async () => {
    await invoke("open_shared_dir");
  }));
  $("#btnOpenData").addEventListener("click", () => withBusy($("#btnOpenData"), async () => {
    await invoke("open_data_dir");
  }));
  $("#btnResetPwd").addEventListener("click", () => {
    $("#newPwd").value = "";
    $("#newPwd2").value = "";
    $("#pwdModal").classList.add("open");
    setTimeout(() => $("#newPwd").focus(), 50);
  });
  $("#pwdCancel").addEventListener("click", () => $("#pwdModal").classList.remove("open"));
  $("#pwdModal").addEventListener("mousedown", (event) => {
    if (event.target === $("#pwdModal")) $("#pwdModal").classList.remove("open");
  });
  $("#pwdOk").addEventListener("click", () => withBusy($("#pwdOk"), async () => {
    const next = $("#newPwd").value;
    if (next !== $("#newPwd2").value) {
      toast("两次输入的新密码不一致", "warning");
      return;
    }
    if (!next && !confirm("留空将清除访问密码，局域网内任何人可匿名访问。确定继续吗？")) return;
    const snap = await invoke("reset_password", { newPassword: next });
    $("#pwdModal").classList.remove("open");
    render(snap);
    toast(next ? "访问密码已重置，网页登录已失效" : "访问密码已清除", "success");
  }));
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") $("#pwdModal").classList.remove("open");
  });
}

function initTheme() {
  const saved = localStorage.getItem("nb_theme");
  const preferDark = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
  const theme = saved === "light" || saved === "dark" ? saved : (preferDark ? "dark" : "light");
  document.documentElement.dataset.theme = theme;
  $("#themeToggle").addEventListener("click", () => {
    const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    localStorage.setItem("nb_theme", next);
  });
}

/* ---------- 浏览器演示模式 ---------- */
const demo = {
  running: true,
  locked: false,
  password: true,
  auto: true,
  port: 8888,
  version: "1.0.0",
};

function demoSnapshot() {
  return {
    version: demo.version,
    running: demo.running,
    port: demo.port,
    bind_lan: true,
    root_dir: "D:\\共享文件",
    data_dir: "C:\\Users\\Public\\NBNetFile",
    has_password: demo.password,
    auto_start: demo.auto,
    locked: demo.locked,
    urls: demo.running
      ? ["http://127.0.0.1:8888", "http://192.168.1.20:8888"]
      : [],
    stats: {
      uptime_secs: demo.running ? 3725 : 0,
      upload_bytes: 104857600,
      download_bytes: 314572800,
      online_clients: demo.running ? 3 : 0,
      active_requests: 0,
    },
    disk_total: 512 * 1024 * 1024 * 1024,
    disk_free: 128 * 1024 * 1024 * 1024,
    log_path: "C:\\Users\\Public\\NBNetFile\\logs\\server.log",
    last_server_error: null,
  };
}

function demoInvoke(cmd, args = {}) {
  if (cmd === "get_snapshot") return Promise.resolve(demoSnapshot());
  if (cmd === "start_server") {
    demo.running = true;
    return Promise.resolve(demoSnapshot());
  }
  if (cmd === "stop_server") {
    demo.running = false;
    return Promise.resolve(demoSnapshot());
  }
  if (cmd === "set_locked") {
    demo.locked = !!args.locked;
    return Promise.resolve(demoSnapshot());
  }
  if (cmd === "reset_password") {
    demo.password = !!args.newPassword;
    return Promise.resolve(demoSnapshot());
  }
  if (cmd === "open_console") {
    demo.running = true;
    return Promise.resolve("http://127.0.0.1:8888/");
  }
  if (cmd === "pick_directory") return Promise.resolve("D:\\共享文件");
  return Promise.resolve(null);
}

if (!hasTauri) {
  $("#demoBanner").classList.remove("hidden");
}

initTheme();
bindActions();
refresh();
setInterval(() => {
  if (document.visibilityState === "visible") refresh();
}, 1200);
