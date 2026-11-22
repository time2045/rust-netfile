import {
  $, initChrome, initDialogs, initScrollRestore, initSidebar, initTheme, setRouteHandler,
  showView, startRouter,
} from "./core.js";
import { applyParams as applyFiles, initFiles } from "./files.js";

initTheme();
initDialogs();
initSidebar();
initChrome();
initFiles();

const restoreScroll = initScrollRestore();
let lastRestore = null;

setRouteHandler((params) => {
  showView("files");
  applyFiles(params);
  clearTimeout(lastRestore);
  lastRestore = setTimeout(() => restoreScroll(), 120);
});

startRouter();

$("#btnLockRefresh")?.addEventListener("click", () => location.reload());
