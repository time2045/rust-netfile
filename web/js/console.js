import {
  $, $$, api, apiText, closeModal, confirmDialog, copyText, el, formatDuration, formatSize,
  icon, navigate, openModal, rootUrl, showLockScreen, sleep, toast,
  NB,
} from "./core.js";

const state = {
  tab: "overview",
  config: null,
  snapshot: null,
  overviewTimer: null,
  overviewLoading: false,
  logsKind: "access",
  logsPage: 1,
  logsPages: 1,
  logsTotal: 0,
  logsTimer: null,
  logAuto: true,
  recycle: [],
  recycleSelected: new Set(),
  nginxText: "",
};

const TABS = ["overview", "settings", "logs", "recycle"];

export function applyParams(params) {
  const tab = TABS.includes(params.get("tab")) ? params.get("tab") : "overview";
  state.tab = tab;
  $$("#consoleTabs button").forEach((button) => {
    button.classList.toggle("active", button.dataset.tab === tab);
  });
  $$("#consoleView .console-panel").forEach((panel) => {
    panel.classList.toggle("hidden", panel.dataset.panel !== tab);
  });
  stopTimers();
  if (tab === "overview") {
    loadOverview();
    state.overviewTimer = setInterval(() => {
      if (state.tab === "overview" && document.visibilityState === "visible") {
        loadOverview({ silent: true });
      }
    }, 3000);
  } else if (tab === "settings") {
    loadConfig();
  } else if (tab === "logs") {
    loadLogs();
    if (state.logAuto) {
      state.logsTimer = setInterval(() => {
        if (state.tab === "logs" && document.visibilityState === "visible") loadLogs({ silent: true });
      }, 3000);
    }
  } else if (tab === "recycle") {
    loadRecycle();
  }
}

function stopTimers() {
  if (state.overviewTimer) { clearInterval(state.overviewTimer); state.overviewTimer = null; }
  if (state.logsTimer) { clearInterval(state.logsTimer); state.logsTimer = null; }
}

async function loadOverview({ silent = false } = {}) {
  if (state.overviewLoading) return;
  state.overviewLoading = true;
  try {
    const data = await api("/api/console/overview");
    state.snapshot = data.snapshot;
    state.config = data.config;
    renderOverview();
  } catch (err) {
    if (!silent && err.status !== 401 && err.status !== 423) toast(err.message || "读取概览失败");
    if (err.status === 0) {
      $("#ovStatus").textContent = "无法连接服务";
      $("#ovDot").className = "status-dot bad";
    }
  } finally {
    state.overviewLoading = false;
  }
}

function renderOverview() {
  const snap = state.snapshot;
  if (!snap) return;
  const locked = snap.locked;
  const running = snap.running;
  $("#ovDot").className = "status-dot " + (locked ? "warn" : running ? "ok" : "bad");
  $("#ovStatus").textContent = locked ? "服务已锁定" : running ? "服务运行中" : "服务已停止";
  $("#ovHint").textContent = snap.urls?.[0]
    ? `${snap.urls[0]} · 共享目录 ${snap.root_dir}`
    : `端口 ${snap.port} · 共享目录 ${snap.root_dir}`;
  $("#sideStatus").textContent = locked ? "服务已锁定" : running ? "服务运行中" : "服务未启动";
  $("#btnRestart").disabled = !running;
  $("#btnLock").disabled = locked || !running;

  $("#statUptime").textContent = running ? formatDuration(snap.stats.uptime_secs) : "—";
  $("#statUpload").textContent = formatSize(snap.stats.upload_bytes || 0);
  $("#statDownload").textContent = formatSize(snap.stats.download_bytes || 0);
  $("#statOnline").textContent = (snap.stats.online_clients || 0) + " 台";

  const total = snap.disk_total || 0;
  const free = snap.disk_free || 0;
  if (total > 0) {
    const used = total - free;
    $("#diskBar").style.width = Math.min(100, (used / total) * 100).toFixed(1) + "%";
    $("#diskText").textContent = `已用 ${formatSize(used)} / 共 ${formatSize(total)} · 剩余 ${formatSize(free)}`;
  } else {
    $("#diskBar").style.width = "0%";
    $("#diskText").textContent = "磁盘信息不可用";
  }

  const urlList = $("#urlList");
  urlList.innerHTML = "";
  if (!snap.urls || !snap.urls.length) {
    urlList.appendChild(el("span", "muted", running ? "仅本机访问" : "服务未运行"));
  } else {
    for (const url of snap.urls) {
      const row = el("div", "url-row");
      const link = el("a", "", url);
      link.href = url;
      link.target = "_blank";
      link.rel = "noopener";
      const copy = el("button", "btn btn-secondary btn-sm");
      copy.type = "button";
      copy.appendChild(icon("i-copy"));
      copy.append("复制");
      copy.addEventListener("click", async () => {
        if (await copyText(url)) toast("地址已复制", "success");
        else toast("复制失败，请手动选择");
      });
      row.append(link, copy);
      urlList.appendChild(row);
    }
  }
  $("#pathRoot").textContent = snap.root_dir;
  $("#pathData").textContent = snap.data_dir;
  $("#pathPort").textContent = String(snap.port) + (snap.bind_lan ? "（允许局域网）" : "（仅本机）");
  $("#lastError").textContent = snap.last_server_error || "无";
  $("#errorCard").classList.toggle("has-error", !!snap.last_server_error);
  // 设置表单中的默认值随快照更新
  if (state.config) fillConfigForm(state.config);
}

function fillConfigForm(config) {
  state.config = config;
  $("#setPort").value = config.port;
  $("#setBind").value = config.bind_lan ? "lan" : "local";
  $("#setRoot").value = config.root_dir;
  $("#setUploadLimit").value = config.upload_limit_mb;
  $("#setConflict").value = config.conflict_mode;
  $("#setZip").value = config.zip_compress;
  $("#setIncludeFolder").checked = !!config.zip_include_folder;
  $("#setRecycleDays").value = config.recycle_days;
  $("#setSessionHours").value = config.session_hours;
  $("#setMaxFailures").value = config.max_login_failures;
  $("#setBlockMinutes").value = config.login_block_minutes;
  $("#setAutoStart").checked = !!config.auto_start;
  $("#currentPwdField").classList.toggle("hidden", !config.has_password);
  if (!$("#ngBackend").value) {
    $("#ngBackend").value = `127.0.0.1:${config.port}`;
  }
}

async function loadConfig() {
  try {
    const data = await api("/api/console/config");
    fillConfigForm(data.config);
  } catch (err) {
    if (err.status !== 401 && err.status !== 423) toast(err.message || "读取配置失败");
  }
}

async function saveSettings(event) {
  event.preventDefault();
  const number = (id) => parseInt($(id).value, 10);
  const payload = {
    port: number("#setPort"),
    bind_lan: $("#setBind").value === "lan",
    root_dir: $("#setRoot").value.trim(),
    upload_limit_mb: number("#setUploadLimit"),
    conflict_mode: $("#setConflict").value,
    zip_compress: $("#setZip").value,
    zip_include_folder: $("#setIncludeFolder").checked,
    recycle_days: number("#setRecycleDays"),
    session_hours: number("#setSessionHours"),
    max_login_failures: number("#setMaxFailures"),
    login_block_minutes: number("#setBlockMinutes"),
    auto_start: $("#setAutoStart").checked,
  };
  if (!payload.root_dir) { toast("共享目录不能为空", "warning"); return; }
  if (!(payload.port >= 1 && payload.port <= 65535)) { toast("端口必须在 1-65535 之间", "warning"); return; }
  const oldPort = state.snapshot?.port;
  const wasRunning = state.snapshot?.running;
  const button = $("#btnSaveSettings");
  button.disabled = true;
  button.textContent = "保存中…";
  try {
    const data = await api("/api/console/config", { method: "POST", body: payload });
    toast(data.message || "配置已保存", "success");
    state.config = { ...state.config, ...payload, has_password: state.config?.has_password };
    if (data.restarting) {
      await waitForRestart(payload.port, oldPort, wasRunning);
    } else {
      await loadOverview({ silent: true });
    }
  } catch (err) {
    toast(err.message);
  } finally {
    button.disabled = false;
    button.textContent = "保存设置";
  }
}

async function changePassword(event) {
  event.preventDefault();
  const current = $("#pwdCurrent").value;
  const next = $("#pwdNew").value;
  const confirm = $("#pwdConfirm").value;
  if (!next) { toast("新密码不能为空", "warning"); return; }
  if (next !== confirm) { toast("两次输入的新密码不一致", "warning"); return; }
  if (state.config?.has_password && !current) { toast("请输入当前密码", "warning"); return; }
  const button = $("#btnSavePassword");
  button.disabled = true;
  button.textContent = "提交中…";
  try {
    const data = await api("/api/console/password", {
      method: "POST",
      body: { current, new_password: next },
    });
    toast(data.message || "密码已更新", "success");
    setTimeout(() => { location.href = rootUrl("/login"); }, 900);
  } catch (err) {
    toast(err.message);
    button.disabled = false;
    button.textContent = "修改密码";
  }
}

async function waitForRestart(newPort, oldPort, wasRunning) {
  const direct = wasRunning && String(location.port || "") === String(oldPort || "");
  let origin = location.origin;
  if (direct && newPort && String(newPort) !== String(oldPort)) {
    origin = location.protocol + "//" + location.hostname + ":" + newPort;
  }
  openModal("restartModal");
  $("#restartHint").textContent = direct && origin !== location.origin
    ? `服务将监听新端口 ${newPort}，完成后会自动跳转。`
    : "请稍候，页面会自动恢复连接。";
  const started = Date.now();
  while (Date.now() - started < 45000) {
    await sleep(700);
    try {
      const response = await fetch(origin + (NB.root || "") + "/api/ping", {
        cache: "no-store", credentials: "same-origin",
      });
      if (response.ok) {
        const data = await response.json();
        if (data.locked) {
          showLockScreen();
          return;
        }
        closeModal("restartModal");
        if (origin !== location.origin) {
          location.href = origin + (NB.root || "") + "/";
        } else {
          toast("服务已重启", "success");
          location.reload();
        }
        return;
      }
    } catch (err) {
      // 服务重启期间连接被拒绝属于预期
    }
  }
  $("#restartHint").textContent = "仍无法连接服务，请检查端口是否被占用，或查看日志。";
}

async function loadLogs({ silent = false } = {}) {
  const query = new URLSearchParams({
    kind: state.logsKind,
    page: String(state.logsPage),
    page_size: "50",
  });
  try {
    const data = await api("/api/console/logs?" + query.toString());
    state.logsPages = data.total_pages || 1;
    state.logsTotal = data.total || 0;
    state.logsPage = data.page || 1;
    renderLogs(data.items || []);
  } catch (err) {
    if (!silent && err.status !== 401 && err.status !== 423) toast(err.message || "读取日志失败");
  }
}

function renderLogs(items) {
  const table = $("#logTable");
  const thead = table.querySelector("thead");
  const tbody = table.querySelector("tbody");
  thead.innerHTML = "";
  tbody.innerHTML = "";
  const headRow = el("tr");
  const isAccess = state.logsKind === "access";
  const columns = isAccess
    ? ["时间", "客户端", "方法", "路径", "状态", "耗时"]
    : ["时间", "文件", "大小", "客户端", "结果"];
  for (const text of columns) {
    const th = el("th", "", text);
    headRow.appendChild(th);
  }
  thead.appendChild(headRow);
  if (!items.length) {
    const row = el("tr");
    const td = el("td", "muted", "暂无记录");
    td.colSpan = columns.length;
    td.style.textAlign = "center";
    td.style.padding = "24px";
    row.appendChild(td);
    tbody.appendChild(row);
  }
  for (const item of items) {
    const row = el("tr");
    if (isAccess) {
      if (item.status >= 400) row.classList.add("fail");
      row.append(
        el("td", "nowrap", item.time),
        el("td", "mono nowrap", item.ip),
        el("td", "mono", item.method),
        el("td", "mono path-cell", item.path),
        el("td", "mono " + (item.status >= 400 ? "status-bad" : "status-ok"), String(item.status)),
        el("td", "nowrap", item.ms + " ms"),
      );
    } else {
      const failed = String(item.result || "").startsWith("失败");
      if (failed) row.classList.add("fail");
      row.append(
        el("td", "nowrap", item.time),
        el("td", "", item.name),
        el("td", "nowrap", item.size ? formatSize(item.size) : "—"),
        el("td", "mono nowrap", item.ip),
        el("td", "", item.result),
      );
    }
    tbody.appendChild(row);
  }
  const pager = $("#logPager");
  pager.innerHTML = "";
  const info = el("div", "pager-info", `共 ${state.logsTotal} 条 · 第 ${state.logsPage} / ${state.logsPages} 页`);
  const buttons = el("div", "pager-btns");
  const prev = el("button", "page-btn" + (state.logsPage <= 1 ? " disabled" : ""), "上一页");
  prev.type = "button";
  prev.addEventListener("click", () => { state.logsPage--; loadLogs(); });
  const next = el("button", "page-btn" + (state.logsPage >= state.logsPages ? " disabled" : ""), "下一页");
  next.type = "button";
  next.addEventListener("click", () => { state.logsPage++; loadLogs(); });
  buttons.append(prev, next);
  pager.append(info, buttons);
}

async function loadRecycle() {
  try {
    const data = await api("/api/console/recycle");
    state.recycle = data.entries || [];
    state.recycleSelected.clear();
    $("#recycleDays").textContent = String(data.days ?? 30);
    renderRecycle();
  } catch (err) {
    if (err.status !== 401 && err.status !== 423) toast(err.message || "读取回收站失败");
  }
}

function renderRecycle() {
  const table = $("#recycleTable");
  const thead = table.querySelector("thead");
  const tbody = table.querySelector("tbody");
  thead.innerHTML = "";
  tbody.innerHTML = "";
  const head = el("tr");
  const checkTh = el("th", "col-check");
  const checkAll = el("input");
  checkAll.type = "checkbox";
  checkAll.addEventListener("change", () => {
    state.recycleSelected.clear();
    if (checkAll.checked) state.recycle.forEach((entry) => state.recycleSelected.add(entry.name));
    renderRecycle();
  });
  checkTh.appendChild(checkAll);
  head.append(checkTh, el("th", "", "名称"), el("th", "", "原位置"), el("th", "", "删除时间"), el("th", "", "大小"), el("th", "", "剩余"));
  thead.appendChild(head);
  $("#recycleEmpty").classList.toggle("hidden", state.recycle.length > 0);
  table.classList.toggle("hidden", state.recycle.length === 0);
  for (const entry of state.recycle) {
    const row = el("tr");
    const tdCheck = el("td", "col-check");
    const check = el("input");
    check.type = "checkbox";
    check.checked = state.recycleSelected.has(entry.name);
    check.addEventListener("change", () => {
      if (check.checked) state.recycleSelected.add(entry.name);
      else state.recycleSelected.delete(entry.name);
    });
    tdCheck.appendChild(check);
    const time = new Date(entry.deleted_ts * 1000);
    const timeText = time.toLocaleString("zh-CN", { hour12: false });
    const days = entry.days_left <= 0 ? "不足 1 天" : entry.days_left + " 天";
    row.append(
      tdCheck,
      el("td", "", entry.name),
      el("td", "path-cell", "/" + entry.orig),
      el("td", "nowrap", timeText),
      el("td", "nowrap", entry.is_dir ? "文件夹" : formatSize(entry.size)),
      el("td", "nowrap", days),
    );
    tbody.appendChild(row);
  }
}

function selectedRecycle() {
  return Array.from(state.recycleSelected);
}

async function recycleAction(kind) {
  const names = selectedRecycle();
  if (kind !== "clear" && !names.length) {
    toast("请先选中要操作的条目", "warning");
    return;
  }
  const confirmText = kind === "restore"
    ? `将选中的 ${names.length} 项还原到原位置？`
    : kind === "purge"
      ? `永久删除选中的 ${names.length} 项？此操作不可恢复。`
      : `清空回收站全部 ${state.recycle.length} 项？此操作不可恢复。`;
  const choice = await confirmDialog({
    title: kind === "restore" ? "还原项目" : "永久删除",
    message: confirmText,
    okText: kind === "restore" ? "还原" : "永久删除",
    danger: kind !== "restore",
  });
  if (choice !== "ok") return;
  try {
    let data;
    if (kind === "clear") data = await api("/api/console/recycle/clear", { method: "POST", body: {} });
    else data = await api("/api/console/recycle/" + kind, { method: "POST", body: { names } });
    toast(data.message || "操作完成", data.failures && data.failures.length ? "warning" : "success");
    loadRecycle();
  } catch (err) {
    toast(err.message);
  }
}

async function generateNginx() {
  const backend = $("#ngBackend").value.trim() || `127.0.0.1:${state.snapshot?.port || 8888}`;
  const prefix = $("#ngPrefix").value.trim();
  const listen = $("#ngListen").value.trim();
  const query = new URLSearchParams({ backend, prefix, listen });
  try {
    const text = await apiText("/api/console/nginx?" + query.toString());
    state.nginxText = text;
    $("#nginxOutput").textContent = text;
    $("#btnCopyNginx").disabled = false;
  } catch (err) {
    toast(err.message || "生成失败");
  }
}

async function doLock() {
  const choice = await confirmDialog({
    title: "锁定服务",
    message: "锁定后所有网页访问将被拒绝，需要在桌面启动台或系统托盘解锁；无桌面环境时需重启服务。\n\n确定锁定吗？",
    okText: "锁定",
  });
  if (choice !== "ok") return;
  try {
    const data = await api("/api/console/lock", { method: "POST", body: {} });
    toast(data.message || "服务已锁定", "warning");
    showLockScreen();
  } catch (err) {
    toast(err.message);
  }
}

async function doRestart() {
  const choice = await confirmDialog({
    title: "重启服务",
    message: "重启期间网页会短暂断开，完成后自动恢复。确定继续吗？",
    okText: "重启",
    danger: false,
  });
  if (choice !== "ok") return;
  const oldPort = state.snapshot?.port;
  try {
    await api("/api/console/restart", { method: "POST", body: {} });
    await sleep(400);
    await waitForRestart(oldPort, oldPort, true);
  } catch (err) {
    toast(err.message);
  }
}

export function initConsole() {
  $$("#consoleTabs button").forEach((button) => {
    button.addEventListener("click", () => {
      navigate({ view: "console", tab: button.dataset.tab });
    });
  });
  $("#settingsForm").addEventListener("submit", saveSettings);
  $("#passwordForm").addEventListener("submit", changePassword);
  $("#btnGenNginx").addEventListener("click", generateNginx);
  $("#btnCopyNginx").addEventListener("click", async () => {
    if (!state.nginxText) return;
    if (await copyText(state.nginxText)) toast("配置已复制", "success");
    else toast("复制失败，请手动选择");
  });
  $("#btnRestart").addEventListener("click", doRestart);
  $("#btnLock").addEventListener("click", doLock);
  $("#btnRefreshRecycle").addEventListener("click", loadRecycle);
  $("#btnRestoreRecycle").addEventListener("click", () => recycleAction("restore"));
  $("#btnPurgeRecycle").addEventListener("click", () => recycleAction("purge"));
  $("#btnClearRecycle").addEventListener("click", () => recycleAction("clear"));
  $$("#logKinds button").forEach((button) => {
    button.addEventListener("click", () => {
      state.logsKind = button.dataset.kind;
      state.logsPage = 1;
      $$("#logKinds button").forEach((node) => node.classList.toggle("active", node === button));
      loadLogs();
    });
  });
  $("#autoLogs").addEventListener("change", () => {
    state.logAuto = $("#autoLogs").checked;
    if (state.logsTimer) { clearInterval(state.logsTimer); state.logsTimer = null; }
    if (state.logAuto && state.tab === "logs") {
      state.logsTimer = setInterval(() => loadLogs({ silent: true }), 3000);
    }
  });
  $("#btnRefreshLogs").addEventListener("click", () => loadLogs());
  $("#btnClearLogs").addEventListener("click", async () => {
    const choice = await confirmDialog({
      title: "清空日志",
      message: "清空访问、上传与下载日志（日志文件也会被清空）？",
      okText: "清空",
    });
    if (choice !== "ok") return;
    try {
      await api("/api/console/logs/clear", { method: "POST", body: {} });
      toast("日志已清空", "success");
      loadLogs();
    } catch (err) {
      toast(err.message);
    }
  });
  $("#btnRetryConnect")?.addEventListener("click", () => location.reload());
}
