import {
  $, initChrome, initDialogs, initScrollRestore, initSidebar, initTheme, setRouteHandler,
  showView, startRouter,
} from "./core.js";
import { applyParams as applyFiles, initFiles } from "./files.js";
import { applyParams as applyConsole, initConsole } from "./console.js";

initTheme();
initDialogs();
initSidebar();
initChrome();
initFiles();
initConsole();

const restoreScroll = initScrollRestore();
let lastRestore = null;

setRouteHandler((params) => {
  const isConsole = params.get("view") === "console";
  showView(isConsole ? "console" : "files");
  if (isConsole) applyConsole(params);
  else applyFiles(params);
  clearTimeout(lastRestore);
  lastRestore = setTimeout(() => restoreScroll(), 120);
});

startRouter();

$("#btnLockRefresh")?.addEventListener("click", () => location.reload());
