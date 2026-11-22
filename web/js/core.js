export const NB = window.NB_APP || { root: "", version: "1.0.0", locked: false, has_password: true };

export const $ = (sel, root = document) => root.querySelector(sel);
export const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

export function el(tag, className = "", text = null) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== null && text !== undefined) node.textContent = text;
  return node;
}

export function icon(name, className = "icon") {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", className);
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", "#" + name);
  svg.appendChild(use);
  return svg;
}

export function rootUrl(path) {
  const base = NB.root || "";
  return base + (path.startsWith("/") ? path : "/" + path);
}

export function readCookie(name) {
  const text = document.cookie || "";
  for (const part of text.split(";")) {
    const [key, ...rest] = part.trim().split("=");
    if (key === name) return decodeURIComponent(rest.join("="));
  }
  return null;
}

export function formatSize(bytes) {
  const value = Number(bytes);
  if (!Number.isFinite(value) || value < 0) return "—";
  if (value < 1024) return Math.round(value) + " B";
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let v = value;
  for (const unit of units) {
    v /= 1024;
    if (v < 1024) return v.toFixed(1) + " " + unit;
  }
  return v.toFixed(1) + " PB";
}

export function formatSpeed(bps) {
  return formatSize(bps) + "/s";
}

export function formatEta(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  const s = Math.round(seconds);
  if (s < 60) return s + " 秒";
  if (s < 3600) return Math.floor(s / 60) + " 分 " + (s % 60) + " 秒";
  if (s < 86400) return Math.floor(s / 3600) + " 时 " + Math.floor((s % 3600) / 60) + " 分";
  return Math.floor(s / 86400) + " 天 " + Math.floor((s % 86400) / 3600) + " 小时";
}

export function formatDuration(seconds) {
  const s = Math.max(0, Math.floor(Number(seconds) || 0));
  const pad = (n) => String(n).padStart(2, "0");
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h >= 24) return Math.floor(h / 24) + " 天 " + (h % 24) + " 小时";
  return pad(h) + ":" + pad(m) + ":" + pad(sec);
}

export function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export function debounce(fn, wait = 250) {
  let timer = null;
  return (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), wait);
  };
}

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (e) {
    const area = el("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    let ok = false;
    try { ok = document.execCommand("copy"); } catch (err) { ok = false; }
    area.remove();
    return ok;
  }
}

export class ApiError extends Error {
  constructor(status, message, data = null) {
    super(message);
    this.status = status;
    this.data = data;
  }
}

let lockShown = false;
export function showLockScreen() {
  if (lockShown) return;
  lockShown = true;
  const screen = $("#lockScreen");
  if (screen) screen.classList.remove("hidden");
}
export function hideLockScreen() {
  lockShown = false;
  const screen = $("#lockScreen");
  if (screen) screen.classList.add("hidden");
}

export async function api(path, opts = {}) {
  const method = (opts.method || "GET").toUpperCase();
  const headers = { ...(opts.headers || {}) };
  let body = opts.body;
  if (method !== "GET" && method !== "HEAD") {
    headers["X-CSRF-Token"] = readCookie("nb_csrf") || "";
  }
  if (body !== undefined && body !== null && typeof body === "object" &&
      !(body instanceof Blob) && !(body instanceof FormData) && !(body instanceof ArrayBuffer)) {
    headers["Content-Type"] = "application/json";
    body = JSON.stringify(body);
  }
  let response;
  try {
    response = await fetch(rootUrl(path), {
      method, headers, body,
      cache: "no-store",
      signal: opts.signal,
      credentials: "same-origin",
    });
  } catch (err) {
    if (err && err.name === "AbortError") throw err;
    throw new ApiError(0, "网络连接异常，请检查服务是否运行");
  }
  if (response.status === 401) {
    location.href = rootUrl("/login");
    throw new ApiError(401, "未登录或登录已失效");
  }
  if (response.status === 423) {
    showLockScreen();
    throw new ApiError(423, "服务已锁定");
  }
  const text = await response.text();
  let data = {};
  if (text) {
    try { data = JSON.parse(text); } catch (e) { data = {}; }
  }
  if (!response.ok) {
    throw new ApiError(response.status, (data && data.message) || "请求失败（" + response.status + "）", data);
  }
  return data;
}

export async function apiText(path) {
  const response = await fetch(rootUrl(path), { cache: "no-store", credentials: "same-origin" });
  if (response.status === 401) { location.href = rootUrl("/login"); throw new ApiError(401, "未登录"); }
  if (response.status === 423) { showLockScreen(); throw new ApiError(423, "服务已锁定"); }
  const text = await response.text();
  if (!response.ok) throw new ApiError(response.status, text || "请求失败");
  return text;
}

export async function ping() {
  const response = await fetch(rootUrl("/api/ping"), { cache: "no-store", credentials: "same-origin" });
  if (!response.ok) throw new ApiError(response.status, "无法连接服务");
  return response.json();
}

export function toast(message, type = "error", timeout = 3800) {
  const box = $("#toastBox");
  if (!box) { window.alert(message); return; }
  const node = el("div", "toast " + type, String(message));
  box.appendChild(node);
  setTimeout(() => {
    node.classList.add("hide");
    setTimeout(() => node.remove(), 300);
  }, timeout);
}

export function openModal(id) {
  const modal = document.getElementById(id);
  if (!modal) return;
  modal.classList.add("open");
  document.body.classList.add("modal-open");
}
export function closeModal(id) {
  const modal = document.getElementById(id);
  if (!modal) return;
  modal.classList.remove("open");
  if (!$(".modal.open")) document.body.classList.remove("modal-open");
}

let confirmResolver = null;
export function confirmDialog({ title = "操作确认", message = "", okText = "确定", danger = true, showRename = false, renameText = "重命名保存" }) {
  const modal = $("#confirmModal");
  if (!modal) return Promise.resolve(null);
  $("#confirmTitle").textContent = title;
  $("#confirmMessage").textContent = message;
  const ok = $("#confirmOk");
  ok.textContent = okText;
  ok.className = "btn " + (danger ? "btn-danger" : "");
  const rename = $("#confirmRename");
  rename.textContent = renameText;
  rename.classList.toggle("hidden", !showRename);
  if (confirmResolver) confirmResolver(null);
  return new Promise((resolve) => {
    confirmResolver = resolve;
    openModal("confirmModal");
  });
}
export function finishConfirm(value) {
  closeModal("confirmModal");
  const resolve = confirmResolver;
  confirmResolver = null;
  if (resolve) resolve(value);
}
export function initDialogs() {
  $("#confirmOk").addEventListener("click", () => finishConfirm("ok"));
  $("#confirmRename").addEventListener("click", () => finishConfirm("rename"));
  $("#confirmCancel").addEventListener("click", () => finishConfirm(null));
  document.addEventListener("click", (event) => {
    const closer = event.target.closest("[data-close]");
    if (closer) {
      event.preventDefault();
      closeModal(closer.getAttribute("data-close"));
    }
  });
  $$(".modal").forEach((modal) => {
    modal.addEventListener("mousedown", (event) => {
      if (event.target !== modal) return;
      if (modal.id === "confirmModal") finishConfirm(null);
      else closeModal(modal.id);
    });
  });
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    const open = $$(".modal.open");
    if (!open.length) return;
    if (open[open.length - 1].id === "confirmModal") {
      finishConfirm(null);
    } else {
      closeModal(open[open.length - 1].id);
    }
  });
  $("#btnLockRefresh")?.addEventListener("click", () => location.reload());
}

let routeHandler = null;
export function setRouteHandler(fn) { routeHandler = fn; }
export function currentParams() { return new URLSearchParams(location.search); }

export function navigate(params, { replace = false, resetScroll = true } = {}) {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== "") search.set(key, String(value));
  }
  const query = search.toString();
  const url = location.pathname + (query ? "?" + query : "");
  if (replace) history.replaceState(null, "", url);
  else history.pushState(null, "", url);
  if (resetScroll) window.scrollTo({ top: 0 });
  if (routeHandler) routeHandler(new URLSearchParams(query));
}

export function startRouter() {
  window.addEventListener("popstate", () => {
    if (routeHandler) routeHandler(currentParams());
  });
  if (routeHandler) routeHandler(currentParams());
}

export function showView(_name) {
  $("#filesView")?.classList.remove("hidden");
  const title = $("#pageTitle");
  if (title) title.textContent = "文件管理";
  $$(".nav-item").forEach((item) => item.classList.toggle("active", item.dataset.nav === "files"));
  document.title = "文件管理 · NB 文件管理器";
}

export function initTheme() {
  const saved = localStorage.getItem("nb_theme");
  const preferDark = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
  const theme = saved === "light" || saved === "dark" ? saved : (preferDark ? "dark" : "light");
  document.documentElement.dataset.theme = theme;
  updateThemeIcon(theme);
  $("#themeToggle")?.addEventListener("click", () => {
    const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    localStorage.setItem("nb_theme", next);
    updateThemeIcon(next);
  });
}
function updateThemeIcon(theme) {
  const use = $("#themeIcon");
  if (use) use.setAttribute("href", theme === "dark" ? "#i-sun" : "#i-moon");
  const btn = $("#themeToggle");
  if (btn) btn.title = theme === "dark" ? "切换到亮色主题" : "切换到暗色主题";
}

export function initSidebar() {
  const sidebar = $("#sidebar");
  const mask = $("#sidebarMask");
  const close = () => {
    sidebar.classList.remove("open");
    mask.classList.add("hidden");
  };
  $("#menuToggle")?.addEventListener("click", () => {
    sidebar.classList.add("open");
    mask.classList.remove("hidden");
  });
  mask?.addEventListener("click", close);
  $$(".nav-item").forEach((item) => item.addEventListener("click", close));
  window.addEventListener("resize", () => {
    if (window.innerWidth > 980) close();
  });
}

export async function logout() {
  try {
    await api("/api/logout", { method: "POST", body: {} });
  } catch (e) {
    if (e.status === 401) return;
  }
  location.href = rootUrl("/login");
}

export function initChrome() {
  $("#sideVersion").textContent = "v" + NB.version;
  $$("#sideLogout, #topLogout").forEach((link) => {
    if (!NB.has_password || NB.has_password === "false") {
      link.classList.add("hidden");
      return;
    }
    link.addEventListener("click", (event) => {
      event.preventDefault();
      logout();
    });
  });
  $$(".nav-item").forEach((item) => {
    item.addEventListener("click", () => navigate({ view: "files" }));
  });
  $("#btnLockRefresh")?.addEventListener("click", () => location.reload());
}

const EXT_CLASS = new Map();
{
  const groups = {
    "ft-image": ["jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "ico", "heic", "tif", "tiff", "avif"],
    "ft-video": ["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts"],
    "ft-audio": ["mp3", "wav", "flac", "aac", "ogg", "m4a", "wma"],
    "ft-archive": ["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso"],
    "ft-pdf": ["pdf"],
    "ft-doc": ["doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "md", "csv", "rtf"],
    "ft-code": ["html", "htm", "js", "ts", "py", "css", "json", "xml", "yml", "yaml", "sh", "bat", "ps1", "c", "cpp", "h", "java", "go", "sql", "toml", "ini", "log"],
  };
  for (const [cls, list] of Object.entries(groups)) {
    for (const ext of list) EXT_CLASS.set(ext, cls);
  }
}

export function fileTypeClass(name, isDir) {
  if (isDir) return "ft-folder";
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return "";
  return EXT_CLASS.get(name.slice(dot + 1).toLowerCase()) || "";
}

export function fileIcon(name, isDir) {
  const svg = icon(isDir ? "i-folder" : "i-file", "icon file-ico " + fileTypeClass(name, isDir));
  return svg;
}

export function extensionOf(name) {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

export function setButtonBusy(button, busy, label = "处理中…") {
  if (!button) return;
  if (busy) {
    if (!button.dataset.originalText) button.dataset.originalText = button.textContent;
    button.disabled = true;
    button.textContent = label;
  } else {
    button.disabled = false;
    if (button.dataset.originalText) {
      button.textContent = button.dataset.originalText;
      delete button.dataset.originalText;
    }
  }
}

export function initScrollRestore() {
  const key = () => "nb_scroll:" + location.pathname + location.search;
  window.addEventListener("pagehide", () => {
    try { sessionStorage.setItem(key(), String(window.scrollY)); } catch (e) {}
  });
  return () => {
    try {
      const saved = parseInt(sessionStorage.getItem(key()) || "0", 10);
      if (saved > 0) requestAnimationFrame(() => window.scrollTo(0, saved));
    } catch (e) {}
  };
}
