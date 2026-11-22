const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => Array.from(document.querySelectorAll(sel));
const hasTauri = !!(window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke);
const invoke = (cmd, args = {}) => hasTauri
  ? window.__TAURI__.core.invoke(cmd, args)
  : demoInvoke(cmd, args);

const PAGE_TITLES = {
  overview: "概览",
  settings: "服务器设置",
  logs: "日志",
  recycle: "回收站",
  about: "关于",
};

let snapshot = null;
let currentPage = "overview";
let settingsDirty = false;
let nginxText = "";
let confirmResolver = null;

const logsState = { kind: "access", page: 1, totalPages: 1, total: 0, timer: null };
const recycleState = { entries: [], selected: new Set(), days: 30 };

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
  }, 3800);
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

function confirmDialog(message, { title = "操作确认", okText = "确定", danger = true } = {}) {
  $("#confirmTitle").textContent = title;
  $("#confirmMessage").textContent = message;
  const ok = $("#confirmOk");
  ok.textContent = okText;
  ok.className = "btn " + (danger ? "danger" : "");
  $("#confirmModal").classList.add("open");
  return new Promise((resolve) => { confirmResolver = resolve; });
}

function finishConfirm(value) {
  $("#confirmModal").classList.remove("open");
  const resolve = confirmResolver;
  confirmResolver = null;
  if (resolve) resolve(value);
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
    if (snapshot) renderSnapshot();
  }
}

function setPage(page) {
  if (!PAGE_TITLES[page]) return;
  currentPage = page;
  $$(".nav-item").forEach((item) => item.classList.toggle("active", item.dataset.page === page));
  $$(".page").forEach((node) => node.classList.toggle("hidden", node.id !== "page" + page[0].toUpperCase() + page.slice(1)));
  $("#pageTitle").textContent = PAGE_TITLES[page];
  try { if (location.hash !== "#" + page) history.replaceState(null, "", "#" + page); } catch (e) {}
  if (logsState.timer) {
    clearInterval(logsState.timer);
    logsState.timer = null;
  }
  if (page === "settings") {
    if (!settingsDirty) fillSettings();
    if (!$("#ngBackend").value && snapshot) $("#ngBackend").value = `127.0.0.1:${snapshot.port}`;
  } else if (page === "logs") {
    loadLogs();
    startLogTimer();
  } else if (page === "recycle") {
    loadRecycle();
  } else if (page === "about") {
    fillAbout();
  }
  if (snapshot) renderSnapshot();
}

async function refreshSnapshot({ silent = false } = {}) {
  try {
    const data = await invoke("get_snapshot");
    snapshot = data;
    renderSnapshot();
  } catch (err) {
    if (!silent) toast("读取服务状态失败：" + errText(err));
    $("#statusBadge").className = "badge bad";
    $("#statusBadge").textContent = "无法连接";
  }
}

function renderSnapshot() {
  if (!snapshot) return;
  const snap = snapshot;
  const running = snap.running;
  const locked = snap.locked;
  $("#sideVersion").textContent = "v" + snap.version;
  const badge = $("#statusBadge");
  badge.className = "badge " + (locked ? "warn" : running ? "ok" : "bad");
  badge.textContent = locked ? "已锁定" : running ? "运行中" : "已停止";
  if (currentPage === "overview") {
    $("#statusDot").className = "dot " + (locked ? "warn" : running ? "ok" : "bad");
    $("#statusTitle").textContent = locked ? "服务已锁定" : running ? "服务运行中" : "服务已停止";
    $("#statusHint").textContent = `${running ? "端口 " + snap.port + " · " : ""}共享目录 ${snap.root_dir}`;
    $("#btnStart").disabled = running;
    $("#btnStop").disabled = !running;
    $("#btnLock").disabled = locked || !running;
    $("#btnUnlock").disabled = !locked;
    renderUrls();
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
    $("#pathRoot").textContent = snap.root_dir;
    $("#pathData").textContent = snap.data_dir;
    $("#pathLog").textContent = snap.log_path;
    $("#lastError").textContent = snap.last_server_error || "无";
    $("#errorCard").classList.toggle("has-error", !!snap.last_server_error);
  }
  $("#pwdState").textContent = snap.has_password ? "已设置" : "未设置（可匿名访问）";
  if (currentPage === "settings" && !settingsDirty) fillSettings();
  if (currentPage === "about") fillAbout();
}

function renderUrls() {
  const list = $("#urlList");
  list.innerHTML = "";
  if (!snapshot.urls || !snapshot.urls.length) {
    const span = document.createElement("span");
    span.className = "muted";
    span.textContent = "服务未运行";
    list.appendChild(span);
  } else {
    for (const url of snapshot.urls) {
      const row = document.createElement("div");
      row.className = "url-row";
      const open = document.createElement("button");
      open.className = "url-link";
      open.textContent = url;
      open.title = "在浏览器中打开";
      open.addEventListener("click", async () => {
        try { await invoke("open_url", { url }); } catch (err) { toast(errText(err)); }
      });
      const copy = document.createElement("button");
      copy.className = "url-copy";
      copy.textContent = "复制";
      copy.addEventListener("click", async () => {
        if (await copyText(url)) toast("地址已复制", "success");
        else toast("复制失败，请手动选择");
      });
      row.append(open, copy);
      list.appendChild(row);
    }
  }
  $("#urlHint").textContent = snapshot.urls && snapshot.urls.length
    ? "局域网设备使用上述地址访问；Web 页只提供文件操作。"
    : "启动服务后可在此复制访问地址。";
}

function fillSettings() {
  if (!snapshot) return;
  const snap = snapshot;
  $("#setPort").value = snap.port;
  $("#setBind").value = snap.bind_lan ? "lan" : "local";
  $("#setRoot").value = snap.root_dir;
  $("#setUploadLimit").value = snap.upload_limit_mb;
  $("#setConflict").value = snap.conflict_mode;
  $("#setZip").value = snap.zip_compress;
  $("#setIncludeFolder").checked = !!snap.zip_include_folder;
  $("#setRecycleDays").value = snap.recycle_days;
  $("#setSessionHours").value = snap.session_hours;
  $("#setMaxFailures").value = snap.max_login_failures;
  $("#setBlockMinutes").value = snap.login_block_minutes;
  $("#setAutoStart").checked = !!snap.auto_start;
  $("#pwdState").textContent = snap.has_password ? "已设置" : "未设置（可匿名访问）";
  if (!$("#ngBackend").value) $("#ngBackend").value = `127.0.0.1:${snap.port}`;
}

function fillAbout() {
  if (!snapshot) return;
  $("#aboutVersion").textContent = "NB 文件管理器 v" + snapshot.version;
  $("#aboutData").textContent = snapshot.data_dir;
  $("#aboutLog").textContent = snapshot.log_path;
  $("#aboutRoot").textContent = snapshot.root_dir;
}

async function saveSettings(event) {
  event.preventDefault();
  const number = (id) => Number($(id).value);
  const payload = {
    port: number("#setPort"),
    bindLan: $("#setBind").value === "lan",
    rootDir: $("#setRoot").value.trim(),
    uploadLimitMb: number("#setUploadLimit"),
    conflictMode: $("#setConflict").value,
    zipCompress: $("#setZip").value,
    zipIncludeFolder: $("#setIncludeFolder").checked,
    recycleDays: number("#setRecycleDays"),
    sessionHours: number("#setSessionHours"),
    maxLoginFailures: number("#setMaxFailures"),
    loginBlockMinutes: number("#setBlockMinutes"),
    autoStart: $("#setAutoStart").checked,
  };
  if (!payload.rootDir) { toast("共享目录不能为空", "warning"); return; }
  const checks = [
    ["监听端口", payload.port, 1, 65535],
    ["上传限制", payload.uploadLimitMb, 0, 10 * 1024 * 1024],
    ["回收站保留天数", payload.recycleDays, 1, 3650],
    ["登录有效期", payload.sessionHours, 1, 720],
    ["登录失败上限", payload.maxLoginFailures, 3, 100],
    ["封禁时长", payload.loginBlockMinutes, 1, 1440],
  ];
  for (const [label, value, min, max] of checks) {
    if (!Number.isInteger(value) || value < min || value > max) {
      toast(`${label}必须是 ${min}-${max} 之间的整数`, "warning");
      return;
    }
  }
  const button = $("#btnSaveSettings");
  button.disabled = true;
  button.textContent = "保存中…";
  try {
    const result = await invoke("save_settings", { settings: payload });
    snapshot = result.snapshot;
    settingsDirty = false;
    toast(result.message || "设置已保存", "success");
    renderSnapshot();
    fillSettings();
  } catch (err) {
    toast(errText(err));
  } finally {
    button.disabled = false;
    button.textContent = "保存设置";
  }
}

async function resetPassword() {
  const next = $("#newPwd").value;
  if (next !== $("#newPwd2").value) { toast("两次输入的新密码不一致", "warning"); return; }
  if (!next) {
    const ok = await confirmDialog("留空将清除访问密码，局域网内任何人可匿名访问。确定继续吗？", {
      title: "清除访问密码", okText: "清除",
    });
    if (!ok) return;
  }
  try {
    const snap = await invoke("reset_password", { newPassword: next });
    snapshot = snap;
    $("#newPwd").value = "";
    $("#newPwd2").value = "";
    renderSnapshot();
    toast(next ? "访问密码已重置，网页登录已失效" : "访问密码已清除", "success");
  } catch (err) {
    toast(errText(err));
  }
}

async function pickRoot() {
  try {
    const picked = await invoke("pick_directory", { current: $("#setRoot").value.trim() });
    if (picked) {
      $("#setRoot").value = picked;
      settingsDirty = true;
    }
  } catch (err) {
    toast(errText(err));
  }
}

async function generateNginx() {
  const backend = $("#ngBackend").value.trim() || `127.0.0.1:${snapshot ? snapshot.port : 8888}`;
  const prefix = $("#ngPrefix").value.trim();
  const listen = $("#ngListen").value.trim();
  try {
    const text = await invoke("generate_nginx", { backend, prefix, listen });
    nginxText = text;
    $("#nginxOutput").textContent = text;
    $("#btnCopyNginx").disabled = false;
  } catch (err) {
    toast(errText(err));
  }
}

function startLogTimer() {
  if (logsState.timer || !$("#autoLogs").checked) return;
  logsState.timer = setInterval(() => {
    if (currentPage === "logs" && document.visibilityState === "visible") loadLogs({ silent: true });
  }, 3000);
}

async function loadLogs({ silent = false } = {}) {
  try {
    const data = await invoke("get_logs", {
      kind: logsState.kind,
      page: logsState.page,
      pageSize: 50,
    });
    logsState.total = data.total || 0;
    logsState.totalPages = data.total_pages || 1;
    logsState.page = data.page || 1;
    renderLogs(data.items || []);
  } catch (err) {
    if (!silent) toast("读取日志失败：" + errText(err));
  }
}

function renderLogs(items) {
  const table = $("#logTable");
  const head = table.querySelector("thead");
  const body = table.querySelector("tbody");
  head.innerHTML = "";
  body.innerHTML = "";
  const access = logsState.kind === "access";
  const columns = access
    ? ["时间", "客户端", "方法", "路径", "状态", "耗时"]
    : ["时间", "文件", "大小", "客户端", "结果"];
  const row = document.createElement("tr");
  for (const text of columns) {
    const th = document.createElement("th");
    th.textContent = text;
    row.appendChild(th);
  }
  head.appendChild(row);
  if (!items.length) {
    const empty = document.createElement("tr");
    const td = document.createElement("td");
    td.colSpan = columns.length;
    td.className = "muted";
    td.style.textAlign = "center";
    td.style.padding = "26px";
    td.textContent = "暂无记录";
    empty.appendChild(td);
    body.appendChild(empty);
  }
  for (const item of items) {
    const tr = document.createElement("tr");
    const cell = (text, cls = "") => {
      const td = document.createElement("td");
      if (cls) td.className = cls;
      td.textContent = text;
      tr.appendChild(td);
      return td;
    };
    if (access) {
      if (item.status >= 400) tr.className = "fail";
      cell(item.time, "nowrap");
      cell(item.ip, "mono nowrap");
      cell(item.method, "mono");
      cell(item.path, "mono path-cell");
      cell(String(item.status), "mono " + (item.status >= 400 ? "status-bad" : "status-ok"));
      cell(item.ms + " ms", "nowrap");
    } else {
      const failed = String(item.result || "").startsWith("失败");
      if (failed) tr.className = "fail";
      cell(item.time, "nowrap");
      cell(item.name);
      cell(item.size ? formatSize(item.size) : "—", "nowrap");
      cell(item.ip, "mono nowrap");
      cell(item.result);
    }
    body.appendChild(tr);
  }
  const pager = $("#logPager");
  pager.innerHTML = "";
  const info = document.createElement("div");
  info.className = "pager-info";
  info.textContent = `共 ${logsState.total} 条 · 第 ${logsState.page} / ${logsState.totalPages} 页`;
  const buttons = document.createElement("div");
  buttons.className = "pager-btns";
  const mk = (label, delta, disabled) => {
    const button = document.createElement("button");
    button.className = "page-btn";
    button.textContent = label;
    button.disabled = disabled;
    button.addEventListener("click", () => {
      logsState.page += delta;
      loadLogs();
    });
    buttons.appendChild(button);
  };
  mk("上一页", -1, logsState.page <= 1);
  mk("下一页", 1, logsState.page >= logsState.totalPages);
  pager.append(info, buttons);
}

function selectKind(kind) {
  logsState.kind = kind;
  logsState.page = 1;
  $$("#logKinds button").forEach((button) => button.classList.toggle("active", button.dataset.kind === kind));
  loadLogs();
}

async function loadRecycle() {
  try {
    const data = await invoke("get_recycle");
    recycleState.entries = data.entries || [];
    recycleState.days = data.days ?? 30;
    recycleState.selected.clear();
    $("#recycleDays").textContent = String(recycleState.days);
    renderRecycle();
  } catch (err) {
    toast("读取回收站失败：" + errText(err));
  }
}

function renderRecycle() {
  const table = $("#recycleTable");
  const head = table.querySelector("thead");
  const body = table.querySelector("tbody");
  head.innerHTML = "";
  body.innerHTML = "";
  const tableRow = document.createElement("tr");
  const checkTh = document.createElement("th");
  checkTh.className = "col-check";
  const checkAll = document.createElement("input");
  checkAll.type = "checkbox";
  checkAll.addEventListener("change", () => {
    recycleState.selected.clear();
    if (checkAll.checked) recycleState.entries.forEach((entry) => recycleState.selected.add(entry.name));
    renderRecycle();
  });
  checkTh.appendChild(checkAll);
  tableRow.append(checkTh);
  for (const text of ["名称", "原位置", "删除时间", "大小", "剩余"]) {
    const th = document.createElement("th");
    th.textContent = text;
    tableRow.appendChild(th);
  }
  head.appendChild(tableRow);
  $("#recycleEmpty").classList.toggle("hidden", recycleState.entries.length > 0);
  table.classList.toggle("hidden", recycleState.entries.length === 0);
  for (const entry of recycleState.entries) {
    const tr = document.createElement("tr");
    const tdCheck = document.createElement("td");
    tdCheck.className = "col-check";
    const check = document.createElement("input");
    check.type = "checkbox";
    check.checked = recycleState.selected.has(entry.name);
    check.addEventListener("change", () => {
      if (check.checked) recycleState.selected.add(entry.name);
      else recycleState.selected.delete(entry.name);
    });
    tdCheck.appendChild(check);
    const time = new Date(entry.deleted_ts * 1000).toLocaleString("zh-CN", { hour12: false });
    const days = entry.days_left <= 0 ? "不足 1 天" : entry.days_left + " 天";
    tr.append(tdCheck);
    for (const [text, cls] of [
      [entry.name, ""],
      ["/" + entry.orig, "path-cell"],
      [time, "nowrap"],
      [entry.is_dir ? "文件夹" : formatSize(entry.size), "nowrap"],
      [days, "nowrap"],
    ]) {
      const td = document.createElement("td");
      if (cls) td.className = cls;
      td.textContent = text;
      tr.appendChild(td);
    }
    body.appendChild(tr);
  }
}

async function recycleAction(kind) {
  const names = Array.from(recycleState.selected);
  if (kind !== "clear" && !names.length) { toast("请先选中要操作的条目", "warning"); return; }
  if (kind === "purge" || kind === "clear") {
    const message = kind === "purge"
      ? `永久删除选中的 ${names.length} 项？此操作不可恢复。`
      : `清空回收站全部 ${recycleState.entries.length} 项？此操作不可恢复。`;
    const ok = await confirmDialog(message, {
      title: kind === "purge" ? "永久删除" : "清空回收站",
      okText: kind === "purge" ? "永久删除" : "清空",
    });
    if (!ok) return;
  }
  try {
    let result;
    if (kind === "restore") result = await invoke("restore_recycle", { names });
    else if (kind === "purge") result = await invoke("purge_recycle", { names });
    else result = await invoke("clear_recycle");
    const failures = result.failures || [];
    toast(`完成：成功 ${result.ok} 项` + (failures.length ? `，失败 ${failures.length} 项` : ""),
      failures.length ? "warning" : "success");
    loadRecycle();
  } catch (err) {
    toast(errText(err));
  }
}

function initChrome() {
  $$(".nav-item").forEach((item) => {
    item.addEventListener("click", () => setPage(item.dataset.page));
  });
  $("#btnStart").addEventListener("click", () => withBusy($("#btnStart"), async () => {
    snapshot = await invoke("start_server");
    renderSnapshot();
    toast("服务已启动", "success");
  }));
  $("#btnStop").addEventListener("click", () => withBusy($("#btnStop"), async () => {
    snapshot = await invoke("stop_server");
    renderSnapshot();
    toast("服务已停止", "info");
  }));
  $("#btnWeb").addEventListener("click", () => withBusy($("#btnWeb"), async () => {
    const url = await invoke("open_web");
    toast("已在浏览器打开：" + url, "success");
  }));
  $("#btnAboutWeb").addEventListener("click", () => withBusy($("#btnAboutWeb"), async () => {
    const url = await invoke("open_web");
    toast("已在浏览器打开：" + url, "success");
  }));
  $("#btnLock").addEventListener("click", async () => {
    const ok = await confirmDialog("锁定后所有网页访问将被拒绝，需要在桌面管理端或托盘解锁。确定锁定吗？", {
      title: "锁定服务", okText: "锁定",
    });
    if (!ok) return;
    withBusy($("#btnLock"), async () => {
      snapshot = await invoke("set_locked", { locked: true });
      renderSnapshot();
      toast("服务已锁定", "warning");
    });
  });
  $("#btnUnlock").addEventListener("click", () => withBusy($("#btnUnlock"), async () => {
    snapshot = await invoke("set_locked", { locked: false });
    renderSnapshot();
    toast("服务已解锁", "success");
  }));
  $("#btnOpenDir").addEventListener("click", () => withBusy($("#btnOpenDir"), () => invoke("open_shared_dir")));
  $("#btnOpenData").addEventListener("click", () => withBusy($("#btnOpenData"), () => invoke("open_data_dir")));
  $("#btnOpenLogs").addEventListener("click", () => withBusy($("#btnOpenLogs"), () => invoke("open_logs_dir")));
  $("#btnAboutData").addEventListener("click", () => withBusy($("#btnAboutData"), () => invoke("open_data_dir")));
  $("#btnAboutLogs").addEventListener("click", () => withBusy($("#btnAboutLogs"), () => invoke("open_logs_dir")));
  $("#btnOpenRepo").addEventListener("click", () => withBusy($("#btnOpenRepo"), () => invoke("open_url", {
    url: "https://github.com/time2045/rust-netfile",
  })));
  $("#btnPickRoot").addEventListener("click", pickRoot);
  $("#settingsForm").addEventListener("submit", saveSettings);
  $("#settingsForm").addEventListener("input", () => { settingsDirty = true; });
  $("#settingsForm").addEventListener("change", () => { settingsDirty = true; });
  $("#btnResetPwd").addEventListener("click", () => withBusy($("#btnResetPwd"), resetPassword));
  $("#btnGenNginx").addEventListener("click", generateNginx);
  $("#btnCopyNginx").addEventListener("click", async () => {
    if (!nginxText) return;
    if (await copyText(nginxText)) toast("配置已复制", "success");
    else toast("复制失败，请手动选择");
  });
  $$("#logKinds button").forEach((button) => {
    button.addEventListener("click", () => selectKind(button.dataset.kind));
  });
  $("#btnRefreshLogs").addEventListener("click", () => loadLogs());
  $("#btnClearLogs").addEventListener("click", async () => {
    const ok = await confirmDialog("清空访问、上传与下载日志（日志文件也会被清空）？", {
      title: "清空日志", okText: "清空",
    });
    if (!ok) return;
    try {
      await invoke("clear_logs");
      toast("日志已清空", "success");
      loadLogs();
    } catch (err) { toast(errText(err)); }
  });
  $("#autoLogs").addEventListener("change", () => {
    if (logsState.timer) { clearInterval(logsState.timer); logsState.timer = null; }
    startLogTimer();
  });
  $("#btnRefreshRecycle").addEventListener("click", loadRecycle);
  $("#btnRestoreRecycle").addEventListener("click", () => recycleAction("restore"));
  $("#btnPurgeRecycle").addEventListener("click", () => recycleAction("purge"));
  $("#btnClearRecycle").addEventListener("click", () => recycleAction("clear"));
  $("#confirmOk").addEventListener("click", () => finishConfirm(true));
  $("#confirmCancel").addEventListener("click", () => finishConfirm(false));
  $("#confirmModal").addEventListener("mousedown", (event) => {
    if (event.target === $("#confirmModal")) finishConfirm(false);
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") finishConfirm(false);
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
  port: 8888,
  bindLan: true,
  rootDir: "D:\\共享文件",
  dataDir: "C:\\Users\\Public\\NBNetFile",
  uploadLimitMb: 0,
  conflictMode: "ask",
  zipCompress: "standard",
  zipIncludeFolder: true,
  recycleDays: 30,
  sessionHours: 8,
  maxLoginFailures: 5,
  loginBlockMinutes: 10,
  autoStart: true,
  version: "1.1.0",
  recycle: [
    { name: "旧报告_20261120_101500", orig: "资料/旧报告.docx", deleted_ts: Date.now() / 1000 - 3600, size: 52840, is_dir: false, days_left: 29 },
    { name: "临时目录_20261119_182000", orig: "临时目录", deleted_ts: Date.now() / 1000 - 90000, size: 0, is_dir: true, days_left: 27 },
  ],
};

function demoSnapshot() {
  return {
    version: demo.version,
    running: demo.running,
    port: demo.port,
    bind_lan: demo.bindLan,
    root_dir: demo.rootDir,
    data_dir: demo.dataDir,
    has_password: demo.password,
    auto_start: demo.autoStart,
    upload_limit_mb: demo.uploadLimitMb,
    conflict_mode: demo.conflictMode,
    zip_compress: demo.zipCompress,
    zip_include_folder: demo.zipIncludeFolder,
    recycle_days: demo.recycleDays,
    session_hours: demo.sessionHours,
    max_login_failures: demo.maxLoginFailures,
    login_block_minutes: demo.loginBlockMinutes,
    locked: demo.locked,
    urls: demo.running ? ["http://127.0.0.1:" + demo.port, "http://192.168.1.20:" + demo.port] : [],
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

function demoLogs(kind, page, pageSize) {
  const now = new Date();
  const time = (offset) => new Date(now.getTime() - offset * 1000).toLocaleString("zh-CN", { hour12: false });
  let items;
  if (kind === "access") {
    items = Array.from({ length: 23 }, (_, i) => ({
      seq: i, time: time(i * 37), ip: "192.168.1.20", method: i % 5 === 0 ? "POST" : "GET",
      path: i % 5 === 0 ? "/api/upload/complete" : "/api/list?path=&page=1", status: i === 7 ? 404 : 200, ms: 1 + (i % 6),
    }));
  } else {
    items = Array.from({ length: 11 }, (_, i) => ({
      seq: i, time: time(i * 90), name: i % 3 === 0 ? "报告.pdf" : "照片.jpg",
      size: 1024 * 1024 * (i + 1) * 3, ip: "192.168.1.20",
      result: i === 4 ? "失败：超出大小限制" : "成功",
    }));
  }
  const totalPages = Math.max(1, Math.ceil(items.length / pageSize));
  const safePage = Math.min(page, totalPages);
  return {
    items: items.slice((safePage - 1) * pageSize, safePage * pageSize),
    total: items.length, total_pages: totalPages, page: safePage, page_size: pageSize,
  };
}

function demoNginx(backend, prefix, listen) {
  const backendSafe = backend.replace(/[^0-9A-Za-z.:\-_\[\]]/g, "") || "127.0.0.1";
  const prefixSafe = "/" + (prefix || "").split("/").filter((s) => s && s !== "." && s !== "..").join("/");
  const p = prefixSafe === "/" ? "" : prefixSafe;
  const port = /^\d+$/.test(listen) ? listen : "9000";
  return `server {\n    listen ${port};\n\n${p ? `    location = ${p} {\n        return 301 ${p}/;\n    }\n\n    location ${p}/ {\n` : "    location / {\n"}        proxy_pass http://${backendSafe}/;\n${p ? `        proxy_set_header X-Forwarded-Prefix ${p};\n` : ""}        proxy_set_header Host $host;\n        proxy_set_header X-Real-IP $remote_addr;\n        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n        proxy_http_version 1.1;\n        proxy_read_timeout 3600s;\n        proxy_send_timeout 3600s;\n        client_max_body_size 0;\n        proxy_request_buffering off;\n    }\n\n    gzip on;\n    gzip_types text/css application/javascript application/json image/svg+xml;\n    gzip_min_length 2048;\n}`;
}

function demoInvoke(cmd, args = {}) {
  if (cmd === "get_snapshot") return Promise.resolve(demoSnapshot());
  if (cmd === "start_server") { demo.running = true; return Promise.resolve(demoSnapshot()); }
  if (cmd === "stop_server") { demo.running = false; return Promise.resolve(demoSnapshot()); }
  if (cmd === "set_locked") { demo.locked = !!args.locked; return Promise.resolve(demoSnapshot()); }
  if (cmd === "save_settings") {
    const s = args.settings || {};
    demo.port = s.port; demo.bindLan = s.bindLan; demo.rootDir = s.rootDir;
    demo.uploadLimitMb = s.uploadLimitMb; demo.conflictMode = s.conflictMode;
    demo.zipCompress = s.zipCompress; demo.zipIncludeFolder = s.zipIncludeFolder;
    demo.recycleDays = s.recycleDays; demo.sessionHours = s.sessionHours;
    demo.maxLoginFailures = s.maxLoginFailures; demo.loginBlockMinutes = s.loginBlockMinutes;
    demo.autoStart = s.autoStart;
    return Promise.resolve({ snapshot: demoSnapshot(), restarting: false, message: "设置已保存并生效" });
  }
  if (cmd === "reset_password") { demo.password = !!args.newPassword; return Promise.resolve(demoSnapshot()); }
  if (cmd === "get_logs") return Promise.resolve(demoLogs(args.kind, args.page || 1, args.pageSize || 50));
  if (cmd === "clear_logs") return Promise.resolve(null);
  if (cmd === "get_recycle") return Promise.resolve({ entries: demo.recycle, days: demo.recycleDays });
  if (cmd === "restore_recycle") { demo.recycle = demo.recycle.filter((e) => !args.names.includes(e.name)); return Promise.resolve({ ok: args.names.length, failures: [] }); }
  if (cmd === "purge_recycle") { demo.recycle = demo.recycle.filter((e) => !args.names.includes(e.name)); return Promise.resolve({ ok: args.names.length, failures: [] }); }
  if (cmd === "clear_recycle") { const n = demo.recycle.length; demo.recycle = []; return Promise.resolve({ ok: n, failures: [] }); }
  if (cmd === "generate_nginx") return Promise.resolve(demoNginx(args.backend || "", args.prefix || "", args.listen || "9000"));
  if (cmd === "pick_directory") return Promise.resolve("D:\\共享文件");
  if (cmd === "open_web") { demo.running = true; return Promise.resolve("http://127.0.0.1:" + demo.port + "/"); }
  return Promise.resolve(null);
}

if (!hasTauri) $("#demoBanner").classList.remove("hidden");

initTheme();
initChrome();
const initialPage = location.hash.replace(/^#/, "");
setPage(PAGE_TITLES[initialPage] ? initialPage : "overview");
refreshSnapshot();
setInterval(() => {
  if (document.visibilityState === "visible") refreshSnapshot({ silent: true });
}, 1200);
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) refreshSnapshot({ silent: true });
});
window.addEventListener("hashchange", () => {
  const page = location.hash.replace(/^#/, "");
  if (PAGE_TITLES[page]) setPage(page);
});
