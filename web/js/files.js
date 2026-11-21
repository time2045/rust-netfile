import {
  NB, $, $$, api, apiText, closeModal, confirmDialog, copyText, debounce, el, extensionOf,
  fileIcon, formatEta, formatSize, formatSpeed, icon, navigate, openModal, ping, readCookie,
  rootUrl, sleep, toast,
} from "./core.js";

const CHUNK_SIZE = 8 * 1024 * 1024;
const MAX_NET_RETRIES = 4;
const MAX_UPLOAD_ROWS = 200;
const CONCURRENCY = 3;

const state = {
  path: "",
  q: "",
  page: 1,
  pageSize: 20,
  sort: "name",
  order: "asc",
  mode: "list",
  items: [],
  total: 0,
  totalPages: 1,
  selection: new Set(),
  truncated: false,
  loadSeq: 0,
  renamePath: "",
  destSources: [],
  destAction: "move",
  destFolders: [],
  destSelected: null,
  destExpanded: new Set([""]),
  uploadRows: [],
  uploadRunning: false,
  uploadStopped: false,
  uploadTotal: 0,
  uploadDone: 0,
  uploadStart: 0,
  activeXhrs: new Set(),
  failedItems: [],
  speedRunning: false,
  chipRunning: false,
};

const PREFS = {
  pageSize: "nb_page_size",
  sort: "nb_sort",
  order: "nb_order",
  mode: "nb_view",
  path: "nb_last_path",
};

function pref(key, fallback) {
  const value = localStorage.getItem(key);
  return value === null ? fallback : value;
}

function clampPageSize(value) {
  const n = parseInt(value, 10);
  return [20, 50, 100, 500, 1000].includes(n) ? n : 20;
}

export function applyParams(params) {
  const hasPath = params.has("path");
  let path = hasPath ? params.get("path") : (sessionStorage.getItem(PREFS.path) || "");
  state.path = (path || "").replace(/\\/g, "/").replace(/^\/+|\/+$/g, "");
  state.q = params.get("q") || "";
  const page = parseInt(params.get("page") || "1", 10);
  state.page = Number.isFinite(page) && page > 0 ? page : 1;
  state.pageSize = clampPageSize(params.get("page_size") || pref(PREFS.pageSize, "20"));
  const sort = params.get("sort") || pref(PREFS.sort, "name");
  state.sort = ["name", "size", "mtime"].includes(sort) ? sort : "name";
  const order = params.get("order") || pref(PREFS.order, "asc");
  state.order = order === "desc" ? "desc" : "asc";
  state.mode = pref(PREFS.mode, "list") === "grid" ? "grid" : "list";
  state.selection.clear();
  state.destSelected = null;
  localStorage.setItem(PREFS.pageSize, String(state.pageSize));
  localStorage.setItem(PREFS.sort, state.sort);
  localStorage.setItem(PREFS.order, state.order);
  sessionStorage.setItem(PREFS.path, state.path);
  $("#searchInput").value = state.q;
  renderBreadcrumbs();
  renderSearchNotice();
  renderModeButtons();
  sortMark();
  renderSelection();
  loadFiles();
}

function urlForListing(overrides = {}) {
  const next = {
    view: "files",
    path: state.path,
    q: state.q,
    page: state.page,
    page_size: state.pageSize,
    sort: state.sort,
    order: state.order,
    ...overrides,
  };
  return next;
}

async function loadFiles({ silent = false } = {}) {
  const seq = ++state.loadSeq;
  if (!silent) $("#tableWrap").classList.add("is-loading");
  try {
    const query = new URLSearchParams({
      path: state.path,
      page: String(state.page),
      page_size: String(state.pageSize),
      sort: state.sort,
      order: state.order,
      q: state.q,
    });
    const data = await api("/api/list?" + query.toString());
    if (seq !== state.loadSeq) return;
    state.items = data.items || [];
    state.total = data.total || 0;
    state.totalPages = data.total_pages || 1;
    state.page = data.page || 1;
    state.truncated = !!data.truncated;
    if (data.breadcrumbs) state.breadcrumbs = data.breadcrumbs;
    renderList();
    renderPager();
    renderSearchNotice();
    renderBreadcrumbs(data.breadcrumbs);
    sessionStorage.setItem(PREFS.path, state.path);
  } catch (err) {
    if (seq !== state.loadSeq) return;
    if (err.status === 404) {
      toast("目录不存在，已返回根目录", "warning");
      navigate({ view: "files", path: "" });
      return;
    }
    if (err.status !== 401 && err.status !== 423) toast(err.message || "读取目录失败");
  } finally {
    if (!silent) $("#tableWrap").classList.remove("is-loading");
  }
}

function renderBreadcrumbs(breadcrumbs) {
  const box = $("#breadcrumbs");
  box.innerHTML = "";
  box.classList.remove("expanded");
  const root = el("a", "", "");
  root.href = "#";
  root.dataset.path = "";
  root.appendChild(icon("i-home"));
  root.append(" 根目录");
  root.addEventListener("click", (event) => {
    event.preventDefault();
    navigate(urlForListing({ path: "", q: "", page: 1 }));
  });
  box.appendChild(root);
  const parts = state.path ? state.path.split("/").filter(Boolean) : [];
  const hiddenFrom = parts.length > 4 ? 1 : -1;
  const hiddenTo = parts.length - 3;
  let collapsed = false;
  let current = "";
  let toggleAdded = false;
  parts.forEach((part, index) => {
    current = current ? current + "/" + part : part;
    const hidden = parts.length > 4 && index >= hiddenFrom && index <= hiddenTo;
    if (hidden && !toggleAdded) {
      const toggle = el("button", "crumb-toggle", "…");
      toggle.type = "button";
      toggle.title = "展开完整路径";
      toggle.addEventListener("click", () => {
        box.classList.add("expanded");
        toggle.remove();
      });
      box.append(el("span", "crumb-sep", "›"), toggle);
      toggleAdded = true;
    }
    const sep = el("span", "crumb-sep " + (hidden ? "crumb-hide" : ""), "›");
    const link = el("a", hidden ? "crumb-hide" : "", part);
    link.href = "#";
    link.dataset.path = current;
    link.addEventListener("click", (event) => {
      event.preventDefault();
      navigate(urlForListing({ path: current, q: "", page: 1 }));
    });
    box.append(sep, link);
  });
  if (state.q) {
    const tag = el("span", "search-tag", "搜索: " + state.q);
    const clear = el("button", "crumb-toggle", "✕");
    clear.type = "button";
    clear.title = "清除搜索";
    clear.addEventListener("click", clearSearch);
    tag.append(" ", clear);
    box.appendChild(tag);
  }
}

function renderSearchNotice() {
  const notice = $("#searchNotice");
  if (!state.q) {
    notice.classList.add("hidden");
    return;
  }
  notice.classList.remove("hidden");
  $("#searchKeyword").textContent = state.q;
  $("#searchTruncated").classList.toggle("hidden", !state.truncated);
}

function renderModeButtons() {
  $$("#viewModeSwitch button").forEach((button) => {
    button.classList.toggle("active", button.dataset.mode === state.mode);
  });
  $("#fileTable").classList.toggle("hidden", state.mode === "grid");
  $("#gridView").classList.toggle("hidden", state.mode !== "grid");
}

function renderList() {
  const body = $("#fileBody");
  const grid = $("#gridView");
  body.innerHTML = "";
  grid.innerHTML = "";
  const empty = $("#emptyState");
  const isEmpty = state.items.length === 0;
  const table = $("#fileTable");
  table.classList.toggle("hidden", isEmpty || state.mode === "grid");
  grid.classList.toggle("hidden", isEmpty || state.mode !== "grid");
  empty.classList.toggle("hidden", !isEmpty);
  $("#emptyTitle").textContent = state.q ? "未找到匹配结果" : "当前目录为空";
  if (isEmpty) {
    renderSelection();
    return;
  }
  for (const item of state.items) {
    body.appendChild(buildRow(item));
    grid.appendChild(buildCard(item));
  }
  sortMark();
  renderSelection();
}

function parentPathOf(item) {
  const parts = item.rel_path.split("/");
  parts.pop();
  return parts.join("/") || "根目录";
}

function downloadUrl(rel) {
  return rootUrl("/api/download?path=" + encodeURIComponent(rel));
}

function inlineUrl(rel) {
  return rootUrl("/api/download?inline=1&path=" + encodeURIComponent(rel));
}

function buildRow(item) {
  const tr = el("tr");
  tr.dataset.path = item.rel_path;
  const tdCheck = el("td", "col-check");
  const check = el("input");
  check.type = "checkbox";
  check.className = "item-check";
  check.dataset.path = item.rel_path;
  check.checked = state.selection.has(item.rel_path);
  check.addEventListener("change", () => toggleSelection(item.rel_path, check.checked));
  tdCheck.appendChild(check);
  tr.appendChild(tdCheck);

  const tdName = el("td", "col-name");
  const nameBox = el("div", "file-name");
  nameBox.appendChild(fileIcon(item.name, item.is_dir));
  const fn = el("div", "fn-text");
  const link = el("a", "", item.name);
  if (item.is_dir) {
    link.href = "#";
    link.addEventListener("click", (event) => {
      event.preventDefault();
      navigate(urlForListing({ path: item.rel_path, q: "", page: 1 }));
    });
  } else {
    link.href = downloadUrl(item.rel_path);
    link.addEventListener("click", (event) => {
      if (previewable(item.name)) {
        event.preventDefault();
        openPreview(item);
      }
    });
  }
  fn.appendChild(link);
  if (state.q) fn.appendChild(el("span", "file-sub", parentPathOf(item)));
  nameBox.appendChild(fn);
  tdName.appendChild(nameBox);
  tr.appendChild(tdName);

  tr.appendChild(el("td", "col-size", item.size || "—"));
  tr.appendChild(el("td", "col-time", item.mtime || "—"));

  const tdActions = el("td", "col-actions");
  const actions = el("div", "row-actions");
  if (item.is_dir) {
    actions.appendChild(actionButton("打包下载", "i-download", "download-folder", item, "btn btn-sm"));
  } else {
    const dl = el("a", "btn btn-sm", "下载");
    dl.href = downloadUrl(item.rel_path);
    actions.appendChild(dl);
    const view = el("button", "btn btn-secondary btn-sm", "预览");
    view.type = "button";
    view.disabled = !previewable(item.name);
    view.addEventListener("click", () => openPreview(item));
    actions.appendChild(view);
  }
  actions.appendChild(actionButton("重命名", "i-edit", "rename", item, "btn btn-secondary btn-sm"));
  actions.appendChild(actionButton("移动", "i-move", "move", item, "btn btn-secondary btn-sm"));
  actions.appendChild(actionButton("复制", "i-copy", "copy", item, "btn btn-secondary btn-sm"));
  actions.appendChild(actionButton("删除", "i-trash", "delete", item, "btn btn-danger btn-sm"));
  tdActions.appendChild(actions);
  tr.appendChild(tdActions);

  check.addEventListener("change", () => tr.classList.toggle("selected", check.checked));
  if (check.checked) tr.classList.add("selected");
  return tr;
}

function actionButton(text, iconName, act, item, className) {
  const button = el("button", className);
  button.type = "button";
  button.dataset.act = act;
  button.dataset.path = item.rel_path;
  button.dataset.name = item.name;
  if (iconName) button.appendChild(icon(iconName));
  button.append(text);
  return button;
}

function buildCard(item) {
  const card = el("div", "grid-card" + (state.selection.has(item.rel_path) ? " selected" : ""));
  const check = el("input", "grid-check");
  check.type = "checkbox";
  check.className = "item-check grid-check";
  check.dataset.path = item.rel_path;
  check.checked = state.selection.has(item.rel_path);
  check.addEventListener("click", (event) => event.stopPropagation());
  check.addEventListener("change", () => {
    toggleSelection(item.rel_path, check.checked);
    card.classList.toggle("selected", check.checked);
  });
  card.appendChild(check);
  const thumb = el("div", "grid-thumb");
  const ext = extensionOf(item.name);
  if (!item.is_dir && ["jpg", "jpeg", "png", "gif", "webp", "bmp", "avif", "ico", "heic"].includes(ext)) {
    const img = el("img");
    img.loading = "lazy";
    img.src = inlineUrl(item.rel_path);
    img.alt = item.name;
    thumb.appendChild(img);
  } else {
    thumb.appendChild(fileIcon(item.name, item.is_dir));
  }
  card.appendChild(thumb);
  const meta = el("div", "grid-meta");
  meta.appendChild(el("div", "grid-name", item.name));
  const sub = el("div", "grid-sub");
  sub.appendChild(el("span", "", item.is_dir ? "文件夹" : (item.size || "")));
  sub.appendChild(el("span", "", item.mtime ? item.mtime.slice(5) : ""));
  meta.appendChild(sub);
  card.appendChild(meta);
  card.addEventListener("click", () => {
    if (item.is_dir) navigate(urlForListing({ path: item.rel_path, q: "", page: 1 }));
    else if (previewable(item.name)) openPreview(item);
    else window.location.href = downloadUrl(item.rel_path);
  });
  return card;
}

function toggleSelection(path, selected) {
  if (selected) state.selection.add(path);
  else state.selection.delete(path);
  renderSelection();
}

function renderSelection() {
  const count = state.selection.size;
  $("#selectionInfo").textContent = count ? "已选 " + count + " 项" : "";
  ["btnBatchDownload", "btnBatchCopy", "btnBatchMove", "btnBatchDelete"].forEach((id) => {
    $("#" + id).classList.toggle("hidden", count === 0);
  });
  const visible = $$("#fileBody .item-check[data-path]");
  const checked = visible.filter((node) => node.checked);
  const all = $("#checkAll");
  all.checked = visible.length > 0 && checked.length === visible.length;
  all.indeterminate = checked.length > 0 && checked.length < visible.length;
}

function sortMark() {
  $$(".sort-btn").forEach((button) => {
    const mark = button.querySelector(".sort-mark");
    if (button.dataset.sort === state.sort) {
      mark.textContent = state.order === "asc" ? "↑" : "↓";
    } else {
      mark.textContent = "";
    }
  });
}

function renderPager() {
  const pager = $("#pager");
  pager.innerHTML = "";
  if (state.total === 0) return;
  const start = (state.page - 1) * state.pageSize + 1;
  const end = Math.min(state.page * state.pageSize, state.total);
  const info = el("div", "pager-info");
  info.append(`共 ${state.total} 项 · 第 ${start}-${end} 项 · 第 ${state.page} / ${state.totalPages} 页`);
  const buttons = el("div", "pager-btns");
  const sizeSelect = el("select", "page-select");
  [20, 50, 100, 500, 1000].forEach((n) => {
    const option = el("option", "", n + " 条/页");
    option.value = String(n);
    option.selected = n === state.pageSize;
    sizeSelect.appendChild(option);
  });
  sizeSelect.addEventListener("change", () => {
    navigate(urlForListing({ page: 1, page_size: sizeSelect.value }));
  });
  buttons.appendChild(sizeSelect);
  const addPage = (label, page, disabled, current) => {
    const button = el("button", "page-btn" + (disabled ? " disabled" : "") + (current ? " current" : ""), label);
    button.type = "button";
    if (!disabled && !current) {
      button.addEventListener("click", () => navigate(urlForListing({ page })));
    }
    buttons.appendChild(button);
  };
  addPage("«", 1, state.page <= 1, false);
  addPage("‹", state.page - 1, state.page <= 1, false);
  addPage(String(state.page), state.page, false, true);
  addPage("›", state.page + 1, state.page >= state.totalPages, false);
  addPage("»", state.totalPages, state.page >= state.totalPages, false);
  pager.append(info, buttons);
}

function previewable(name) {
  const ext = extensionOf(name);
  return [
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "avif", "ico",
    "mp4", "webm", "ogv", "mov", "m4v",
    "mp3", "wav", "ogg", "m4a", "flac", "aac",
    "txt", "md", "log", "json", "xml", "csv", "yml", "yaml", "toml", "ini", "conf", "cfg",
    "rs", "go", "py", "js", "ts", "css", "html", "htm", "sh", "bat", "ps1", "sql", "c", "cpp", "h", "java", "php", "vue",
  ].includes(ext);
}

async function openPreview(item) {
  const ext = extensionOf(item.name);
  $("#previewName").textContent = item.name;
  $("#previewMeta").textContent = [item.size || "文件夹", item.mtime, "/" + item.rel_path].filter(Boolean).join(" · ");
  $("#previewDownload").href = downloadUrl(item.rel_path);
  const body = $("#previewBody");
  body.innerHTML = "";
  openModal("previewModal");
  const imageExts = ["jpg", "jpeg", "png", "gif", "webp", "bmp", "avif", "ico"];
  const videoExts = ["mp4", "webm", "ogv", "mov", "m4v"];
  const audioExts = ["mp3", "wav", "ogg", "m4a", "flac", "aac"];
  if (imageExts.includes(ext)) {
    const img = el("img");
    img.src = inlineUrl(item.rel_path);
    img.alt = item.name;
    body.appendChild(img);
    return;
  }
  if (videoExts.includes(ext)) {
    const video = el("video");
    video.controls = true;
    video.autoplay = false;
    video.src = inlineUrl(item.rel_path);
    body.appendChild(video);
    return;
  }
  if (audioExts.includes(ext)) {
    const audio = el("audio");
    audio.controls = true;
    audio.src = inlineUrl(item.rel_path);
    body.appendChild(audio);
    return;
  }
  const loading = el("div", "preview-placeholder", "正在加载文本预览…");
  body.appendChild(loading);
  try {
    const text = await apiText("/api/preview?path=" + encodeURIComponent(item.rel_path));
    body.innerHTML = "";
    body.appendChild(el("pre", "", text));
  } catch (err) {
    body.innerHTML = "";
    body.appendChild(el("div", "preview-placeholder", err.message || "无法预览该文件"));
  }
}

function stopPreviewMedia() {
  const body = $("#previewBody");
  if (!body) return;
  $$("audio, video", body).forEach((node) => {
    try { node.pause(); } catch (e) {}
    node.removeAttribute("src");
  });
}

function initPreviewCleanup() {
  const modal = $("#previewModal");
  new MutationObserver(() => {
    if (!modal.classList.contains("open")) stopPreviewMedia();
  }).observe(modal, { attributes: true, attributeFilter: ["class"] });
}

function clearSearch() {
  navigate(urlForListing({ q: "", page: 1 }));
}

function setRename(path, name) {
  state.renamePath = path;
  $("#renameInput").value = name;
  openModal("renameModal");
  setTimeout(() => $("#renameInput").select(), 50);
}

async function doRename() {
  const name = $("#renameInput").value.trim();
  if (!name) { toast("请输入新名称", "warning"); return; }
  try {
    await api("/api/rename", { method: "POST", body: { path: state.renamePath, new_name: name } });
    closeModal("renameModal");
    toast("重命名成功", "success");
    loadFiles();
  } catch (err) {
    toast(err.message);
  }
}

async function doMkdir() {
  const name = $("#newFolderName").value.trim();
  if (!name) { toast("请输入文件夹名称", "warning"); return; }
  try {
    await api("/api/mkdir", { method: "POST", body: { current_path: state.path, name } });
    closeModal("mkdirModal");
    $("#newFolderName").value = "";
    toast("文件夹已创建", "success");
    loadFiles();
  } catch (err) {
    toast(err.message);
  }
}

function selectedPaths() {
  return Array.from(state.selection);
}

async function deletePaths(paths) {
  if (!paths.length) return;
  const choice = await confirmDialog({
    title: "删除确认",
    message: `确定删除选中的 ${paths.length} 项？\n删除后将移入回收站，可在控制台还原。`,
    okText: "删除",
  });
  if (choice !== "ok") return;
  showTask("正在删除…");
  try {
    const body = paths.length === 1 ? { path: paths[0] } : { paths };
    const endpoint = paths.length === 1 ? "/api/delete" : "/api/delete_many";
    const result = await api(endpoint, { method: "POST", body });
    const done = await waitTask(result.task_id);
    hideTask();
    if (!done) return;
    toast(done.message || "删除完成", "success");
    state.selection.clear();
    loadFiles();
  } catch (err) {
    hideTask();
    toast(err.message);
  }
}

async function downloadFolder(path) {
  showTask("正在打包文件夹…");
  try {
    const result = await api("/api/download_folder", { method: "POST", body: { path } });
    const done = await waitTask(result.task_id);
    if (!done) { hideTask(); return; }
    setTaskText("打包完成，开始下载…");
    window.location.href = rootUrl("/api/download_file?task_id=" + encodeURIComponent(result.task_id));
    setTimeout(hideTask, 2500);
  } catch (err) {
    hideTask();
    toast(err.message);
  }
}

async function downloadMany(paths) {
  if (!paths.length) return;
  const choice = await confirmDialog({
    title: "打包下载",
    message: `确定打包下载选中的 ${paths.length} 个项目？`,
    okText: "开始打包",
    danger: false,
  });
  if (choice !== "ok") return;
  showTask("正在打包…");
  try {
    const result = await api("/api/download_many", { method: "POST", body: { paths } });
    const done = await waitTask(result.task_id);
    if (!done) { hideTask(); return; }
    setTaskText("打包完成，开始下载…");
    window.location.href = rootUrl("/api/download_file?task_id=" + encodeURIComponent(result.task_id));
    setTimeout(hideTask, 2500);
  } catch (err) {
    hideTask();
    toast(err.message);
  }
}

function showTask(text) {
  let panel = $("#taskPanel");
  if (!panel) {
    panel = el("div", "task-panel");
    panel.id = "taskPanel";
    const head = el("div", "task-head");
    const title = el("b", "", "任务进行中");
    title.id = "taskTitle";
    const bar = el("div", "task-bar");
    const inner = el("span");
    inner.id = "taskBar";
    bar.appendChild(inner);
    const textNode = el("div", "task-text");
    textNode.id = "taskText";
    panel.append(head, title, bar, textNode);
    document.body.appendChild(panel);
  }
  setTaskText(text);
  $("#taskBar").style.width = "2%";
  panel.classList.remove("hidden");
}

function setTaskText(text) {
  const node = $("#taskText");
  if (node) node.textContent = text;
}

function setTaskProgress(done, total, message) {
  const bar = $("#taskBar");
  if (bar) bar.style.width = total > 0 ? Math.min(100, Math.round(done / total * 100)) + "%" : "35%";
  if (message) setTaskText(message);
}

function hideTask() {
  const panel = $("#taskPanel");
  if (panel) panel.classList.add("hidden");
}

async function waitTask(taskId, onProgress) {
  let misses = 0;
  while (true) {
    try {
      const data = await api("/api/task/status?task_id=" + encodeURIComponent(taskId));
      if (onProgress) onProgress(data);
      if (data.status === "done") return data;
      if (data.status === "failed") { toast(data.message || "操作失败"); return null; }
    } catch (err) {
      if (err.status === 404) {
        toast("任务不存在或已过期");
        return null;
      }
      if (++misses >= 5) { toast(err.message || "查询任务状态失败"); return null; }
    }
    await sleep(700);
  }
}

async function openMoveModal(paths, action) {
  state.destSources = paths;
  state.destAction = action;
  state.destSelected = null;
  state.destExpanded = new Set([""]);
  $("#moveTitle").textContent = action === "move" ? "移动到文件夹" : "复制到文件夹";
  $("#destFilter").value = "";
  openModal("moveModal");
  await loadDests();
}

async function loadDests() {
  const tree = $("#destTree");
  tree.innerHTML = '<div class="tree-empty">加载中…</div>';
  try {
    const exclude = encodeURIComponent(state.destSources.join(","));
    const data = await api("/api/folders?exclude=" + exclude);
    state.destFolders = data.folders || [];
    renderDestTree();
  } catch (err) {
    tree.innerHTML = "";
    tree.appendChild(el("div", "tree-empty", err.message || "加载文件夹失败"));
  }
}

function buildDestTree() {
  const nodes = new Map();
  let root = null;
  for (const folder of state.destFolders) {
    nodes.set(folder.path, { ...folder, children: [] });
  }
  for (const folder of state.destFolders) {
    const node = nodes.get(folder.path);
    if (folder.depth === 0) { root = node; continue; }
    const index = folder.path.lastIndexOf("/");
    const parent = nodes.get(index === -1 ? "" : folder.path.slice(0, index));
    if (parent) parent.children.push(node);
  }
  return root;
}

function renderDestTree() {
  const box = $("#destTree");
  const filter = $("#destFilter").value.trim().toLowerCase();
  box.innerHTML = "";
  if (!state.destFolders.length) {
    box.appendChild(el("div", "tree-empty", "没有可选的文件夹"));
    return;
  }
  if (filter) {
    const hits = state.destFolders.filter((f) =>
      f.name.toLowerCase().includes(filter) || ("/" + f.path).toLowerCase().includes(filter));
    if (!hits.length) {
      box.appendChild(el("div", "tree-empty", "无匹配文件夹"));
      return;
    }
    hits.slice(0, 300).forEach((folder) => {
      const row = el("div", "tree-row" + (folder.path === state.destSelected ? " selected" : ""));
      row.appendChild(icon("i-folder", "icon tree-icon"));
      const name = el("span", "", folder.name);
      const path = el("span", "tree-path", "/" + (folder.path || ""));
      const wrap = el("span", "", "");
      wrap.append(name, path);
      row.appendChild(wrap);
      row.addEventListener("click", () => { state.destSelected = folder.path; renderDestTree(); });
      box.appendChild(row);
    });
    return;
  }
  const root = buildDestTree();
  if (!root) return;
  const append = (node, depth) => {
    const row = el("div", "tree-row" + (node.path === state.destSelected ? " selected" : ""));
    row.style.paddingLeft = (8 + depth * 16) + "px";
    const hasChildren = node.children.length > 0;
    const arrow = el("span", "tree-arrow" + (hasChildren ? "" : " hidden-arrow"),
      hasChildren && state.destExpanded.has(node.path) ? "▼" : "▶");
    if (hasChildren) {
      arrow.addEventListener("click", (event) => {
        event.stopPropagation();
        if (state.destExpanded.has(node.path)) state.destExpanded.delete(node.path);
        else state.destExpanded.add(node.path);
        renderDestTree();
      });
    }
    row.append(arrow, icon("i-folder", "icon tree-icon"), el("span", "", node.name));
    row.addEventListener("click", () => { state.destSelected = node.path; renderDestTree(); });
    box.appendChild(row);
    if (hasChildren && state.destExpanded.has(node.path)) {
      node.children.forEach((child) => append(child, depth + 1));
    }
  };
  append(root, 0);
}

async function submitMoveCopy() {
  if (state.destSelected === null) {
    toast("请先选择目标文件夹", "warning");
    return;
  }
  const action = state.destAction;
  const sources = state.destSources;
  const dest = state.destSelected;
  closeModal("moveModal");
  showTask(action === "move" ? "正在移动…" : "正在复制…");
  try {
    let endpoint;
    let body;
    if (sources.length > 1) {
      endpoint = action === "move" ? "/api/move_many" : "/api/copy_many";
      body = { srcs: sources, dest_dir: dest };
    } else {
      endpoint = action === "move" ? "/api/move" : "/api/copy";
      body = { src: sources[0], dest_dir: dest };
    }
    const result = await api(endpoint, { method: "POST", body });
    if (result.task_id) {
      const done = await waitTask(result.task_id, (data) => setTaskProgress(data.done, data.total, data.message));
      hideTask();
      if (!done) return;
      toast(done.message || "操作完成", "success");
    } else {
      hideTask();
      toast("操作完成", "success");
    }
    state.selection.clear();
    loadFiles();
  } catch (err) {
    hideTask();
    toast(err.message);
  }
}

function bindTableEvents() {
  $("#fileBody").addEventListener("click", (event) => {
    const button = event.target.closest("button[data-act]");
    if (!button) return;
    const item = { rel_path: button.dataset.path, name: button.dataset.name };
    const act = button.dataset.act;
    if (act === "rename") setRename(item.rel_path, item.name);
    else if (act === "move") openMoveModal([item.rel_path], "move");
    else if (act === "copy") openMoveModal([item.rel_path], "copy");
    else if (act === "delete") deletePaths([item.rel_path]);
    else if (act === "download-folder") downloadFolder(item.rel_path);
  });
  $("#checkAll").addEventListener("change", (event) => {
    const checked = event.target.checked;
    state.selection.clear();
    $$("#fileBody .item-check[data-path]").forEach((node) => {
      node.checked = checked;
      if (checked) state.selection.add(node.dataset.path);
      const row = node.closest("tr");
      if (row) row.classList.toggle("selected", checked);
    });
    renderSelection();
  });
}

function bindToolbar() {
  $("#btnUploadFiles").addEventListener("click", () => $("#fileInput").click());
  $("#btnUploadFolder").addEventListener("click", () => $("#folderInput").click());
  $("#btnNewFolder").addEventListener("click", () => {
    $("#newFolderName").value = "";
    openModal("mkdirModal");
    setTimeout(() => $("#newFolderName").focus(), 50);
  });
  $("#btnCreateFolder").addEventListener("click", doMkdir);
  $("#newFolderName").addEventListener("keydown", (event) => {
    if (event.key === "Enter") { event.preventDefault(); doMkdir(); }
  });
  $("#btnRename").addEventListener("click", doRename);
  $("#renameInput").addEventListener("keydown", (event) => {
    if (event.key === "Enter") { event.preventDefault(); doRename(); }
  });
  $("#fileInput").addEventListener("change", (event) => {
    uploadFiles(event.target.files);
    event.target.value = "";
  });
  $("#folderInput").addEventListener("change", (event) => {
    uploadFiles(event.target.files);
    event.target.value = "";
  });
  $("#btnRefresh").addEventListener("click", () => loadFiles());
  $("#searchForm").addEventListener("submit", (event) => {
    event.preventDefault();
    const q = $("#searchInput").value.trim();
    navigate(urlForListing({ q, page: 1 }));
  });
  $("#btnClearSearch").addEventListener("click", clearSearch);
  $("#btnBatchDownload").addEventListener("click", () => downloadMany(selectedPaths()));
  $("#btnBatchDelete").addEventListener("click", () => deletePaths(selectedPaths()));
  $("#btnBatchMove").addEventListener("click", () => openMoveModal(selectedPaths(), "move"));
  $("#btnBatchCopy").addEventListener("click", () => openMoveModal(selectedPaths(), "copy"));
  $$("#viewModeSwitch button").forEach((button) => {
    button.addEventListener("click", () => {
      state.mode = button.dataset.mode === "grid" ? "grid" : "list";
      localStorage.setItem(PREFS.mode, state.mode);
      renderModeButtons();
      renderList();
    });
  });
  $$(".sort-btn").forEach((button) => {
    button.addEventListener("click", () => {
      const sort = button.dataset.sort;
      let order = "asc";
      if (state.sort === sort) order = state.order === "asc" ? "desc" : "asc";
      navigate(urlForListing({ sort, order, page: 1 }));
    });
  });
  $("#btnReloadDests").addEventListener("click", loadDests);
  $("#destFilter").addEventListener("input", debounce(renderDestTree, 120));
  $("#btnMoveOk").addEventListener("click", submitMoveCopy);
}

function bindDragAndDrop() {
  const overlay = $("#dropOverlay");
  let depth = 0;
  window.addEventListener("dragenter", (event) => {
    event.preventDefault();
    depth++;
    overlay.classList.remove("hidden");
  });
  window.addEventListener("dragover", (event) => event.preventDefault());
  window.addEventListener("dragleave", (event) => {
    event.preventDefault();
    depth = Math.max(0, depth - 1);
    if (depth === 0) overlay.classList.add("hidden");
  });
  window.addEventListener("drop", async (event) => {
    event.preventDefault();
    depth = 0;
    overlay.classList.add("hidden");
    const items = event.dataTransfer?.items;
    if (!items || !items.length) return;
    const files = [];
    const relMap = new Map();
    async function walk(entry, prefix) {
      if (entry.isFile) {
        const file = await new Promise((resolve, reject) => entry.file(resolve, reject));
        files.push(file);
        relMap.set(file, prefix + file.name);
      } else if (entry.isDirectory) {
        const reader = entry.createReader();
        const children = [];
        while (true) {
          const batch = await new Promise((resolve, reject) => reader.readEntries(resolve, reject));
          if (!batch.length) break;
          children.push(...batch);
        }
        for (const child of children) await walk(child, prefix + entry.name + "/");
      }
    }
    const roots = [];
    for (const item of items) {
      const entry = item.webkitGetAsEntry && item.webkitGetAsEntry();
      if (entry) roots.push(entry);
      else if (item.kind === "file") {
        const file = item.getAsFile();
        if (file) { files.push(file); relMap.set(file, file.name); }
      }
    }
    for (const entry of roots) await walk(entry, "");
    if (files.length) uploadFiles(files, relMap);
  });
}

function bindPaste() {
  document.addEventListener("paste", (event) => {
    const data = event.clipboardData;
    if (!data || !data.items) return;
    const files = [];
    const stamp = () => {
      const d = new Date();
      const p = (n, l = 2) => String(n).padStart(l, "0");
      return d.getFullYear() + p(d.getMonth() + 1) + p(d.getDate()) + "_" + p(d.getHours()) + p(d.getMinutes()) + p(d.getSeconds()) + p(d.getMilliseconds(), 3);
    };
    for (const item of data.items) {
      if (item.kind !== "file") continue;
      let file = item.getAsFile();
      if (!file) continue;
      if (!file.name || /^image\.(png|jpe?g)$/i.test(file.name) || file.name === "blob") {
        const ext = ((file.type.split("/")[1] || "png").toLowerCase()).replace("jpeg", "jpg");
        file = new File([file], "截图_" + stamp() + "." + ext, { type: file.type });
      }
      files.push(file);
    }
    if (!files.length) return;
    event.preventDefault();
    toast(`检测到 ${files.length} 个剪贴板文件，开始上传`, "info");
    uploadFiles(files);
  }, true);
}

function fileUploadId(rel, file) {
  const seed = state.path + "|" + rel + "|" + file.size + "|" + (file.lastModified || 0);
  let h = 0x811c9dc5;
  let g = 0x9e3779b9;
  for (let i = 0; i < seed.length; i++) {
    const c = seed.charCodeAt(i);
    h = Math.imul(h ^ c, 0x01000193) >>> 0;
    g = Math.imul(g + c, 0x85ebca6b) >>> 0;
  }
  let x = (h ^ seed.length) >>> 0;
  let out = "";
  for (let i = 0; i < 6; i++) {
    x = Math.imul(x ^ (x >>> 15), 0x2545f491) >>> 0;
    out += (x >>> 0).toString(16).padStart(8, "0");
  }
  return "u" + out + (g >>> 0).toString(16).padStart(8, "0");
}

function uploadStatus(uploadId, rel, total) {
  const query = new URLSearchParams({
    upload_id: uploadId,
    rel_path: rel,
    current_path: state.path,
    total_size: String(total),
  });
  return api("/api/upload/status?" + query.toString())
    .then((data) => (typeof data.offset === "number" ? data.offset : 0))
    .catch(() => 0);
}

function sendChunk(row, uploadId, offset, end, onProgress) {
  return new Promise((resolve) => {
    const xhr = new XMLHttpRequest();
    row.xhr = xhr;
    state.activeXhrs.add(xhr);
    const query = new URLSearchParams({
      upload_id: uploadId,
      rel_path: row.rel,
      current_path: state.path,
      total_size: String(row.file.size),
      offset: String(offset),
    });
    let lastProgress = Date.now();
    let watchdogFired = false;
    const watchdog = setInterval(() => {
      if (Date.now() - lastProgress > 45000) {
        watchdogFired = true;
        try { xhr.abort(); } catch (e) {}
      }
    }, 5000);
    const cleanup = () => {
      state.activeXhrs.delete(xhr);
      clearInterval(watchdog);
      row.xhr = null;
    };
    xhr.upload.onprogress = (event) => {
      lastProgress = Date.now();
      onProgress(offset + event.loaded);
    };
    xhr.onload = () => {
      cleanup();
      let data = {};
      try { data = JSON.parse(xhr.responseText); } catch (e) {}
      if (xhr.status === 200 && data.success) {
        resolve({ ok: true, offset: typeof data.offset === "number" ? data.offset : end });
      } else if (xhr.status === 409) {
        resolve({ ok: false, offset: typeof data.offset === "number" ? data.offset : undefined, message: data.message });
      } else if (xhr.status === 413) {
        resolve({ ok: false, perm: true, message: data.message || "超出大小限制" });
      } else if (xhr.status === 401) {
        location.href = rootUrl("/login");
        resolve({ ok: false, abort: true });
      } else if (xhr.status === 423) {
        resolve({ ok: false, abort: true });
      } else if (xhr.status >= 400 && xhr.status < 500) {
        resolve({ ok: false, perm: true, message: data.message || "请求被拒绝（" + xhr.status + "）" });
      } else {
        resolve({ ok: false, net: true });
      }
    };
    xhr.onerror = () => { cleanup(); resolve({ ok: false, net: true }); };
    xhr.onabort = () => {
      cleanup();
      if (watchdogFired) resolve({ ok: false, timeout: true });
      else resolve({ ok: false, abort: true });
    };
    xhr.open("POST", rootUrl("/api/upload/chunk?" + query.toString()));
    xhr.setRequestHeader("X-CSRF-Token", readCookie("nb_csrf") || "");
    xhr.setRequestHeader("Content-Type", "application/octet-stream");
    xhr.send(row.file.slice(offset, end));
  });
}

async function completeUpload(uploadId, file, rel, mode) {
  const response = await fetch(rootUrl("/api/upload/complete"), {
    method: "POST",
    credentials: "same-origin",
    headers: {
      "Content-Type": "application/json",
      "X-CSRF-Token": readCookie("nb_csrf") || "",
    },
    body: JSON.stringify({
      upload_id: uploadId,
      rel_path: rel,
      current_path: state.path,
      total_size: file.size,
      mode,
    }),
  });
  if (response.status === 401) { location.href = rootUrl("/login"); throw new Error("未登录"); }
  if (response.status === 423) throw new Error("服务已锁定");
  let data = {};
  try { data = await response.json(); } catch (e) {}
  return { status: response.status, data };
}

function abortUpload(uploadId) {
  fetch(rootUrl("/api/upload/abort"), {
    method: "POST",
    credentials: "same-origin",
    headers: {
      "Content-Type": "application/json",
      "X-CSRF-Token": readCookie("nb_csrf") || "",
    },
    body: JSON.stringify({ upload_id: uploadId }),
  }).catch(() => {});
}

async function uploadOne(row, mode) {
  const uploadId = fileUploadId(row.rel, row.file);
  const total = row.file.size;
  let offset = 0;
  let retries = 0;
  let settled = false;
  const setStatus = (text, cls = "") => {
    row.statusEl.textContent = text;
    row.statusEl.className = "u-status" + (cls ? " " + cls : "");
  };
  const setBar = (bytes) => {
    const percent = total > 0 ? Math.round(bytes / total * 100) : 100;
    row.barEl.style.width = percent + "%";
    row.loaded = Math.min(bytes, total);
  };
  const finish = (outcome, label, cls = "") => {
    if (settled) return outcome;
    settled = true;
    if (label) setStatus(label, cls);
    row.outcome = outcome;
    return outcome;
  };

  offset = await uploadStatus(uploadId, row.rel, total);
  row.loaded = Math.min(offset, total);
  setBar(row.loaded);
  if (offset > 0 && offset < total) setStatus("继续上传…");

  while (true) {
    if (state.uploadStopped) {
      abortUpload(uploadId);
      return finish("cancel", "已取消");
    }
    if (offset >= total) break;
    const end = Math.min(offset + CHUNK_SIZE, total);
    const result = await sendChunk(row, uploadId, offset, end, (sent) => {
      row.loaded = sent;
      setBar(sent);
      setStatus("上传中 " + Math.round(total > 0 ? sent / total * 100 : 100) + "%");
      updateUploadProgress();
    });
    if (settled) return "cancel";
    if (result.ok) {
      offset = result.offset;
      retries = 0;
      row.loaded = offset;
      setBar(offset);
      updateUploadProgress();
      continue;
    }
    if (typeof result.offset === "number" && result.offset !== offset) {
      offset = result.offset;
      retries = 0;
      continue;
    }
    if (result.perm) {
      return finish("fail", (result.message || "请求被拒绝").slice(0, 16), "fail");
    }
    if (result.timeout) return finish("fail", "连接超时", "fail");
    if (result.abort) {
      if (state.uploadStopped) abortUpload(uploadId);
      return finish("cancel", "已取消");
    }
    retries++;
    if (retries <= MAX_NET_RETRIES) {
      setStatus(`网络不稳，重试 ${retries}/${MAX_NET_RETRIES}`);
      await sleep(Math.min(600 * retries, 3000));
      offset = await uploadStatus(uploadId, row.rel, total);
      continue;
    }
    return finish("fail", "网络错误", "fail");
  }

  setBar(total);
  setStatus("正在保存…");
  try {
    let response;
    try {
      response = await completeUpload(uploadId, row.file, row.rel, mode);
    } catch (err) {
      return finish("fail", err.message || "保存失败", "fail");
    }
    if (response.status === 200 && response.data.success) {
      row.loaded = total;
      row.barEl.style.width = "100%";
      return finish("ok", "完成", "ok");
    }
    if (response.status === 409) {
      const data = response.data || {};
      if (data.conflict) return finish("fail", "同名冲突", "fail");
      if (typeof data.offset === "number") {
        offset = Math.min(data.offset, total);
        retries = 0;
        return uploadOne(row, mode);
      }
      return finish("fail", (data.message || "同名冲突").slice(0, 16), "fail");
    }
    if (response.status === 413) return finish("fail", "超出大小限制", "fail");
    if (response.status >= 400 && response.status < 500) {
      return finish("fail", (response.data.message || "请求被拒绝").slice(0, 16), "fail");
    }
    return finish("fail", "服务器错误 " + response.status, "fail");
  } finally {
    // 不清理服务端暂存：保留断点，刷新后可续传
  }
}

function addUploadRow(rel, file) {
  if (state.uploadRows.length >= MAX_UPLOAD_ROWS) {
    const stub = {
      rel, file,
      statusEl: { textContent: "", className: "" },
      barEl: { style: {} },
      loaded: 0, outcome: null, xhr: null,
    };
    state.uploadRows.push(stub);
    return stub;
  }
  const row = el("div", "upload-row");
  const index = el("div", "u-index", String(state.uploadRows.length + 1) + ".");
  const name = el("div", "u-name", rel);
  const status = el("div", "u-status", "等待中");
  const bar = el("div", "u-bar");
  const inner = el("span");
  bar.appendChild(inner);
  row.append(index, name, status, bar);
  $("#uploadList").appendChild(row);
  const item = { rel, file, statusEl: status, barEl: inner, loaded: 0, outcome: null, xhr: null };
  row.__row = item;
  state.uploadRows.push(item);
  return item;
}

function updateUploadProgress() {
  const total = state.uploadTotal || 1;
  let done = 0;
  for (const row of state.uploadRows) {
    done += row.outcome === "ok" ? row.file.size : Math.min(row.loaded || 0, row.file.size);
  }
  state.uploadDone = done;
  const percent = Math.min(100, Math.round(done / total * 100));
  $("#globalBar").style.width = percent + "%";
  const elapsed = Math.max(0.001, (Date.now() - state.uploadStart) / 1000);
  const speed = done / elapsed;
  const remain = Math.max(0, total - done);
  $("#globalText").textContent = `${percent}% · ${formatSpeed(speed)} · 剩余 ${formatEta(speed > 0 ? remain / speed : Infinity)}（${formatSize(done)} / ${formatSize(total)}）`;
}

async function uploadFiles(fileList, relMap) {
  const files = Array.from(fileList || []);
  if (!files.length) return;
  if (state.uploadRunning) {
    toast("已有上传任务进行中，请等待完成", "warning");
    return;
  }
  const relOf = (file) => (relMap && relMap.get(file)) || file.webkitRelativePath || file.name;
  const limit = Number(NB.upload_limit_bytes || 0);
  const accepted = [];
  const oversize = [];
  for (const file of files) {
    if (limit > 0 && file.size > limit) oversize.push(file);
    else accepted.push(file);
  }
  $("#uploadPanel").classList.remove("hidden");
  $("#uploadList").style.display = "";
  $("#btnCollapseQueue").textContent = "收起";
  if (oversize.length) {
    for (const file of oversize) {
      const row = addUploadRow(relOf(file), file);
      if (!row) break;
      row.statusEl.textContent = "超出大小限制";
      row.statusEl.className = "u-status fail";
      row.barEl.style.width = "100%";
      row.barEl.style.background = "var(--danger)";
      row.outcome = "fail";
    }
    toast(`已跳过 ${oversize.length} 个文件：超过大小限制（${formatSize(limit)}）`, "error");
  }
  if (!accepted.length) {
    $("#uploadPanel").classList.remove("hidden");
    return;
  }

  let mode = "ask";
  const configured = NB.conflict_mode || "ask";
  if (configured === "ask") {
    try {
      const rels = accepted.map(relOf);
      const result = await api("/api/check_conflicts", {
        method: "POST",
        body: { current_path: state.path, rel_paths: rels },
      });
      if (result.conflicts && result.conflicts.length) {
        const names = result.conflicts.slice(0, 12).join("、");
        const more = result.conflicts.length > 12 ? ` 等 ${result.conflicts.length} 个` : "";
        const choice = await confirmDialog({
          title: "检测到同名项目",
          message: `以下 ${result.conflicts.length} 个项目已存在：\n${names}${more}\n\n请选择处理方式：`,
          okText: "覆盖",
          showRename: true,
          danger: true,
        });
        if (choice === null) {
          $("#uploadPanel").classList.add("hidden");
          return;
        }
        mode = choice === "rename" ? "rename" : "overwrite";
      }
    } catch (err) {
      toast(err.message);
      return;
    }
  } else {
    mode = configured;
  }

  state.uploadStopped = false;
  state.uploadRunning = true;
  state.failedItems = [];
  state.uploadRows = [];
  $("#uploadList").innerHTML = "";
  $("#uploadPanel").classList.remove("hidden");
  const rows = accepted.map((file) => addUploadRow(relOf(file), file)).filter(Boolean);
  const totalBytes = accepted.reduce((sum, file) => sum + file.size, 0);
  state.uploadTotal = totalBytes;
  state.uploadDone = 0;
  state.uploadStart = Date.now();
  updateUploadProgress();
  $("#uploadCount").textContent = "0 / " + accepted.length;
  $("#uploadCount").classList.remove("hidden");
  $("#globalBar").style.width = "0%";
  $("#globalText").textContent = "准备上传…";
  $("#btnStopUpload").classList.remove("hidden");
  $("#btnRetryFailed").classList.add("hidden");
  $("#btnCollapseQueue").textContent = "收起";
  $("#uploadList").style.display = "";

  let next = 0;
  let completed = 0;
  let success = 0;
  let failed = 0;
  const worker = async () => {
    while (true) {
      const index = next++;
      if (index >= rows.length) return;
      const row = rows[index];
      if (state.uploadStopped) {
        row.statusEl.textContent = "已取消";
        row.outcome = "cancel";
        continue;
      }
      row.statusEl.textContent = "上传中";
      const outcome = await uploadOne(row, mode);
      if (outcome === "ok") {
        success++;
        state.uploadDone += row.file.size;
      } else {
        if (outcome === "fail") {
          failed++;
          state.failedItems.push({ file: row.file, rel: row.rel });
        }
      }
      completed++;
      $("#uploadCount").textContent = `${success} / ${accepted.length}`;
      updateUploadProgress();
    }
  };
  const workerCount = Math.min(CONCURRENCY, rows.length);
  try {
    await Promise.all(Array.from({ length: workerCount }, () => worker()));
  } finally {
    state.uploadRunning = false;
  }

  $("#btnStopUpload").classList.add("hidden");
  state.activeXhrs.clear();
  if (state.uploadStopped) {
    $("#globalText").textContent = "已停止上传（未完成的文件未保存）";
    toast("已停止上传", "info");
  } else if (failed > 0) {
    $("#globalText").textContent = `上传结束：${success} 成功，${failed} 失败`;
    $("#btnRetryFailed").classList.remove("hidden");
    toast(`有 ${failed} 个文件上传失败，可点击重试`, "error");
  } else {
    $("#globalText").textContent = "上传完成";
    toast("上传完成", "success");
  }
  $("#uploadCount").classList.add("hidden");
  state.uploadStopped = false;
  if (success > 0) loadFiles({ silent: true });
}

function initUploadControls() {
  $("#btnStopUpload").addEventListener("click", () => {
    if (!state.uploadRunning) return;
    state.uploadStopped = true;
    for (const xhr of [...state.activeXhrs]) {
      try { xhr.abort(); } catch (e) {}
    }
  });
  $("#btnRetryFailed").addEventListener("click", () => {
    const failed = state.failedItems.slice();
    if (!failed.length) return;
    $("#btnRetryFailed").classList.add("hidden");
    state.failedItems = [];
    const files = failed.map((item) => item.file);
    const map = new Map(failed.map((item) => [item.file, item.rel]));
    uploadFiles(files, map);
  });
  $("#btnClearQueue").addEventListener("click", () => {
    $$("#uploadList .upload-row").forEach((node) => {
      const text = node.querySelector(".u-status")?.textContent || "";
      if (["完成", "已取消", "超出大小限制", "同名冲突"].includes(text.trim())) {
        node.remove();
      }
    });
    if (!$("#uploadList .upload-row")) $("#uploadPanel").classList.add("hidden");
  });
  $("#btnCollapseQueue").addEventListener("click", () => {
    const list = $("#uploadList");
    const hidden = list.style.display === "none";
    list.style.display = hidden ? "" : "none";
    $("#btnCollapseQueue").textContent = hidden ? "收起" : "展开";
  });
  window.addEventListener("beforeunload", (event) => {
    if (state.activeXhrs.size > 0) {
      event.preventDefault();
      event.returnValue = "";
    }
  });
}

function initSpeedTest() {
  const chip = $("#netChip");
  const dot = $("#netDot");
  const text = $("#netText");
  const level = (ms) => {
    if (ms <= 30) return ["ok", "局域网级连接"];
    if (ms <= 80) return ["ok", "连接良好"];
    if (ms <= 200) return ["mid", "网络一般"];
    return ["bad", "弱网状态"];
  };
  async function measurePing(n = 4) {
    const values = [];
    for (let i = 0; i < n; i++) {
      const started = performance.now();
      await ping();
      values.push(performance.now() - started);
    }
    const avg = Math.round(values.reduce((a, b) => a + b, 0) / values.length);
    const jitter = Math.round(Math.max(...values) - Math.min(...values));
    return { avg, jitter };
  }
  async function refreshChip() {
    if (state.chipRunning || state.speedRunning) return;
    state.chipRunning = true;
    try {
      const { avg } = await measurePing(3);
      const [cls, label] = level(avg);
      dot.className = "net-dot " + cls;
      text.textContent = avg + " ms";
      chip.title = label + " · 点击打开网络测速";
    } catch (e) {
      dot.className = "net-dot bad";
      text.textContent = "离线";
    } finally {
      state.chipRunning = false;
    }
  }
  chip.addEventListener("click", () => {
    openModal("speedModal");
    $("#speedPing").textContent = "—";
    $("#speedDown").textContent = "—";
    $("#speedUp").textContent = "—";
    $("#speedPingSub").textContent = "";
    $("#speedDownSub").textContent = "";
    $("#speedUpSub").textContent = "";
    $("#speedVerdict").textContent = "";
    $("#speedBar").style.width = "0%";
  });
  async function measureDown(seconds = 5) {
    const started = performance.now();
    let bytes = 0;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), (seconds + 3) * 1000);
    try {
      const response = await fetch(rootUrl("/api/speed/down?bytes=" + 64 * 1024 * 1024), {
        signal: controller.signal, cache: "no-store", credentials: "same-origin",
      });
      const reader = response.body.getReader();
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        bytes += value.byteLength;
        const elapsed = performance.now() - started;
        if (elapsed > 200) {
          $("#speedDown").textContent = ((bytes * 8 / (elapsed / 1000)) / 1e6).toFixed(1) + " Mbps";
          $("#speedBar").style.width = Math.min(50, elapsed / (seconds * 1000) * 50) + "%";
        }
        if (elapsed >= seconds * 1000) { try { await reader.cancel(); } catch (e) {} break; }
      }
    } catch (e) {
    } finally {
      clearTimeout(timer);
    }
    const elapsed = Math.max(0.1, (performance.now() - started) / 1000);
    return { bytes, bps: bytes / elapsed, mbps: bytes * 8 / elapsed / 1e6 };
  }
  async function measureUp(seconds = 5) {
    const started = performance.now();
    let sent = 0;
    const blob = new Blob([new Uint8Array(8 * 1024 * 1024)]);
    while (performance.now() - started < seconds * 1000) {
      const remain = seconds * 1000 - (performance.now() - started);
      const ok = await new Promise((resolve) => {
        const xhr = new XMLHttpRequest();
        let finished = false;
        const done = (value) => { if (!finished) { finished = true; resolve(value); } };
        const timer = setTimeout(() => { try { xhr.abort(); } catch (e) {} done(false); }, Math.max(500, remain));
        xhr.upload.onprogress = (event) => {
          const elapsed = performance.now() - started;
          if (elapsed > 200) {
            $("#speedUp").textContent = (((sent + event.loaded) * 8 / (elapsed / 1000)) / 1e6).toFixed(1) + " Mbps";
            $("#speedBar").style.width = Math.min(100, 50 + elapsed / (seconds * 1000) * 50) + "%";
          }
        };
        xhr.onload = () => { clearTimeout(timer); done(xhr.status === 200); };
        xhr.onerror = () => { clearTimeout(timer); done(false); };
        xhr.onabort = () => { clearTimeout(timer); done(false); };
        xhr.open("POST", rootUrl("/api/speed/up"));
        xhr.setRequestHeader("X-CSRF-Token", readCookie("nb_csrf") || "");
        xhr.send(blob);
      });
      if (ok) sent += blob.size;
      else break;
      if (performance.now() - started >= seconds * 1000) break;
    }
    const elapsed = Math.max(0.1, (performance.now() - started) / 1000);
    return { bytes: sent, bps: sent / elapsed, mbps: sent * 8 / elapsed / 1e6 };
  }
  $("#btnRunSpeed").addEventListener("click", async () => {
    if (state.speedRunning) return;
    state.speedRunning = true;
    const button = $("#btnRunSpeed");
    button.disabled = true;
    button.textContent = "测速中…";
    $("#speedVerdict").textContent = "";
    $("#speedBar").style.width = "0%";
    try {
      $("#speedPing").textContent = "测量中…";
      const pingResult = await measurePing(5);
      $("#speedPing").textContent = pingResult.avg + " ms";
      $("#speedPingSub").textContent = "抖动 ±" + pingResult.jitter + " ms";
      $("#speedDown").textContent = "…";
      const down = await measureDown();
      if (down.bytes > 0) {
        $("#speedDown").textContent = down.mbps.toFixed(1) + " Mbps";
        $("#speedDownSub").textContent = formatSpeed(down.bps) + " · 共 " + formatSize(down.bytes);
      } else {
        $("#speedDown").textContent = "失败";
      }
      $("#speedUp").textContent = "…";
      const up = await measureUp();
      if (up.bytes > 0) {
        $("#speedUp").textContent = up.mbps.toFixed(1) + " Mbps";
        $("#speedUpSub").textContent = formatSpeed(up.bps) + " · 共 " + formatSize(up.bytes);
      } else {
        $("#speedUp").textContent = "失败";
      }
      $("#speedBar").style.width = "100%";
      let verdict;
      if (down.bytes > 0) {
        if (down.mbps >= 50) verdict = "下载速度很快，适合大文件传输";
        else if (down.mbps >= 10) verdict = "下载速度良好，日常使用流畅";
        else if (down.mbps >= 1) verdict = "下载速度一般，大文件传输可能偏慢";
        else verdict = "下载带宽极低，请检查网络";
      } else {
        verdict = level(pingResult.avg)[1];
      }
      if (up.bytes > 0 && up.mbps < 1) verdict += "；上传带宽极低，建议靠近信号源或检查网络";
      $("#speedVerdict").textContent = verdict;
      refreshChip();
    } catch (err) {
      $("#speedVerdict").textContent = "测速失败：" + (err.message || "网络异常");
    } finally {
      state.speedRunning = false;
      button.disabled = false;
      button.textContent = "开始测速";
    }
  });
  refreshChip();
  setInterval(refreshChip, 30000);
}

export function initFiles() {
  bindTableEvents();
  bindToolbar();
  bindDragAndDrop();
  bindPaste();
  initUploadControls();
  initSpeedTest();
  initPreviewCleanup();
  $("#searchInput").addEventListener("keydown", (event) => {
    if (event.key === "Escape") clearSearch();
  });
}
