(function () {
  const root = (window.NB_APP && window.NB_APP.root) || "";
  const version = (window.NB_APP && window.NB_APP.version) || "";
  const form = document.getElementById("loginForm");
  const password = document.getElementById("password");
  const button = document.getElementById("submitBtn");
  const errorBox = document.getElementById("errorBox");
  const countdown = document.getElementById("countdown");
  const lockNotice = document.getElementById("lockNotice");
  let locked = false;
  let blocked = false;
  let timer = null;

  function cookie(name) {
    for (const part of (document.cookie || "").split(";")) {
      const [key, ...rest] = part.trim().split("=");
      if (key === name) return decodeURIComponent(rest.join("="));
    }
    return "";
  }

  function setTheme(theme) {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("nb_theme", theme);
  }

  const saved = localStorage.getItem("nb_theme");
  const preferDark = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
  const theme = saved === "light" || saved === "dark" ? saved : (preferDark ? "dark" : "light");
  document.documentElement.dataset.theme = theme;
  document.getElementById("themeToggle").addEventListener("click", () => {
    setTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark");
  });

  if (version) document.getElementById("version").textContent = "NB 文件管理器 v" + version;

  function showError(message) {
    errorBox.textContent = message;
    errorBox.classList.remove("hidden");
  }

  function hideError() {
    errorBox.classList.add("hidden");
  }

  function showLocked() {
    locked = true;
    lockNotice.classList.remove("hidden");
    password.disabled = true;
    button.disabled = true;
    button.textContent = "服务已锁定";
  }

  function startCountdown(seconds, message) {
    blocked = true;
    if (timer) clearInterval(timer);
    let left = Math.max(1, parseInt(seconds, 10) || 60);
    hideError();
    password.value = "";
    countdown.classList.remove("hidden");
    password.disabled = true;
    button.disabled = true;
    const render = () => {
      countdown.textContent = `尝试次数过多，请在 ${left} 秒后重试。`;
      button.textContent = `${left} 秒后可重试`;
    };
    render();
    timer = setInterval(() => {
      left--;
      if (left <= 0) {
        clearInterval(timer);
        timer = null;
        blocked = false;
        countdown.classList.add("hidden");
        password.disabled = false;
        button.disabled = false;
        button.textContent = "进入系统";
        password.focus();
      } else {
        render();
      }
    }, 1000);
  }

  async function ensureCsrf() {
    if (cookie("nb_csrf")) return;
    try {
      await fetch(root + "/api/ping", { cache: "no-store" });
    } catch (e) {}
  }

  async function checkLocked() {
    try {
      const response = await fetch(root + "/api/ping", { cache: "no-store" });
      if (!response.ok) return;
      const data = await response.json();
      if (data.locked) showLocked();
    } catch (e) {}
  }

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (locked || blocked) return;
    hideError();
    const value = password.value;
    if (!value) {
      showError("请输入访问密码");
      password.focus();
      return;
    }
    button.disabled = true;
    button.textContent = "验证中…";
    try {
      const response = await fetch(root + "/api/login", {
        method: "POST",
        credentials: "same-origin",
        headers: {
          "Content-Type": "application/json",
          "X-CSRF-Token": cookie("nb_csrf"),
        },
        body: JSON.stringify({ password: value }),
      });
      let data = {};
      try { data = await response.json(); } catch (e) {}
      if (response.ok && data.success) {
        button.textContent = "正在进入…";
        location.href = data.redirect || (root + "/");
        return;
      }
      if (response.status === 429) {
        startCountdown(data.retry_after || 60, data.message || "尝试次数过多");
        return;
      }
      if (response.status === 423) {
        showLocked();
        return;
      }
      if (response.status === 403) {
        await ensureCsrf();
        showError("页面已过期，请刷新后重试");
      } else {
        showError(data.message || "密码错误，请重新输入。");
      }
      password.value = "";
      password.focus();
    } catch (err) {
      showError("网络连接异常，请检查服务是否运行");
    } finally {
      if (!blocked && !locked) {
        button.disabled = false;
        button.textContent = "进入系统";
      }
    }
  });

  ensureCsrf().then(checkLocked);
})();
