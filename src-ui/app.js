const tauri = window.__TAURI__;
const invoke = tauri?.core?.invoke;
const listen = tauri?.event?.listen;
const TRAY_HINT_SEEN_KEY = "n2n-supernode-launcher.tray-hint-seen";

const els = {
  tabs: Array.from(document.querySelectorAll(".nav-tab")),
  pages: Array.from(document.querySelectorAll(".page")),
  homeLogCard: document.querySelector("#homeLogCard"),
  port: document.querySelector("#portInput"),
  managementPort: document.querySelector("#managementPortInput"),
  extraArgs: document.querySelector("#extraArgsInput"),
  autoScroll: document.querySelector("#autoScrollInput"),
  fastReconnect: document.querySelector("#fastReconnectInput"),
  closeBehaviorSegments: Array.from(document.querySelectorAll(".segment")),
  trayHintModal: document.querySelector("#trayHintModal"),
  trayHintConfirm: document.querySelector("#trayHintConfirmButton"),
  start: document.querySelector("#startButton"),
  stop: document.querySelector("#stopButton"),
  save: document.querySelector("#saveButton"),
  clearLogs: document.querySelector("#clearLogsButton"),
  statusText: document.querySelector("#statusText"),
  pidText: document.querySelector("#pidText"),
  logView: document.querySelector("#logView"),
};

let autoScroll = true;
let runtimeRunning = false;
let closeBehavior = "exit";
let saveTimer = null;

function selectTab(tabName) {
  const pageName = tabName === "settings" ? "settings" : "home";
  els.tabs.forEach((tab) => {
    tab.classList.toggle("active", tab.dataset.tab === tabName);
  });
  els.pages.forEach((page) => {
    page.classList.toggle("active", page.dataset.page === pageName);
  });
}

function hasSeenTrayHint() {
  return window.localStorage.getItem(TRAY_HINT_SEEN_KEY) === "1";
}

function markTrayHintSeen() {
  window.localStorage.setItem(TRAY_HINT_SEEN_KEY, "1");
}

function showTrayHint() {
  els.trayHintModal.classList.remove("hidden");
  els.trayHintModal.setAttribute("aria-hidden", "false");
}

function hideTrayHint() {
  els.trayHintModal.classList.add("hidden");
  els.trayHintModal.setAttribute("aria-hidden", "true");
}

function renderEmptyLog() {
  if (els.logView.childElementCount === 0) {
    const empty = document.createElement("div");
    empty.className = "log-empty";
    empty.textContent = "请启动或重新连接以显示日志";
    els.logView.append(empty);
  }
}

function clearEmptyLog() {
  const empty = els.logView.querySelector(".log-empty");
  if (empty) {
    empty.remove();
  }
}

function appendLog(entry) {
  clearEmptyLog();
  const row = document.createElement("div");
  row.className = `log-line ${entry.stream || "stdout"}`;
  row.innerHTML = `
    <span class="time">${entry.timestamp || "--:--:--"}</span>
    <span class="stream">${entry.stream || "stdout"}</span>
    <span class="message"></span>
  `;
  row.querySelector(".message").textContent = entry.message ?? "";
  els.logView.append(row);
  if (autoScroll) {
    els.logView.scrollTop = els.logView.scrollHeight;
  }
}

function setRunningState(running, status, pid) {
  runtimeRunning = running;
  els.statusText.textContent = status || (running ? "运行中" : "未启动");
  els.pidText.textContent = `PID: ${pid ?? "-"}`;
  els.start.disabled = running;
  els.stop.disabled = !running;
  els.statusText.classList.toggle("running", running);
}

function syncCloseBehaviorUI(value) {
  closeBehavior = value;
  els.closeBehaviorSegments.forEach((segment) => {
    segment.classList.toggle("active", segment.dataset.closeBehavior === value);
  });
}

function readForm() {
  return {
    port: els.port.value.trim(),
    managementPort: els.managementPort.value.trim(),
    extraArgs: els.extraArgs.value.trim(),
    autoScroll: els.autoScroll.checked,
    allowFastReconnect: els.fastReconnect.checked,
    closeBehavior,
  };
}

function applyForm(config) {
  els.port.value = config.port ?? "7654";
  els.managementPort.value = config.managementPort ?? "5645";
  els.extraArgs.value = config.extraArgs ?? "-f";
  els.autoScroll.checked = config.autoScroll ?? true;
  els.fastReconnect.checked = config.allowFastReconnect ?? false;
  syncCloseBehaviorUI(config.closeBehavior ?? "exit");
  autoScroll = els.autoScroll.checked;
}

async function saveSettings({ silent = false } = {}) {
  const config = readForm();
  await invoke("save_settings", { config });
  if (!silent) {
    appendLog({
      timestamp: now(),
      stream: "system",
      message: "配置已保存",
    });
  }
}

function scheduleSilentSave() {
  window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    saveSettings({ silent: true }).catch((error) => {
      appendLog({ timestamp: now(), stream: "system", message: `保存配置失败: ${String(error)}` });
    });
  }, 180);
}

async function refreshStatus() {
  const snapshot = await invoke("refresh_status");
  setRunningState(snapshot.running, snapshot.status, snapshot.pid);
}

async function startSupernode() {
  const config = readForm();
  const snapshot = await invoke("start_supernode", { config });
  setRunningState(snapshot.running, snapshot.status, snapshot.pid);
}

async function stopSupernode() {
  const snapshot = await invoke("stop_supernode");
  setRunningState(snapshot.running, snapshot.status, snapshot.pid);
}

function now() {
  return new Date().toLocaleTimeString("zh-CN", { hour12: false });
}

async function init() {
  if (!invoke || !listen) {
    appendLog({
      timestamp: now(),
      stream: "system",
      message: "未检测到 Tauri API，当前页面需要通过 Tauri 应用运行。",
    });
    setRunningState(false, "无法连接 Tauri", null);
    return;
  }

  renderEmptyLog();
  selectTab("home");
  applyForm(await invoke("load_settings"));
  await refreshStatus();

  await listen("supernode-log", (event) => appendLog(event.payload));
  await listen("supernode-status", (event) => {
    const payload = event.payload;
    setRunningState(payload.running, payload.status, payload.pid);
  });

  els.tabs.forEach((tab) => {
    tab.addEventListener("click", () => selectTab(tab.dataset.tab));
  });

  els.start.addEventListener("click", async () => {
    try {
      await startSupernode();
      els.homeLogCard?.scrollIntoView({ behavior: "smooth", block: "start" });
    } catch (error) {
      appendLog({ timestamp: now(), stream: "system", message: String(error) });
    }
  });

  els.stop.addEventListener("click", async () => {
    try {
      await stopSupernode();
    } catch (error) {
      appendLog({ timestamp: now(), stream: "system", message: String(error) });
    }
  });

  els.save.addEventListener("click", async () => {
    try {
      await saveSettings();
    } catch (error) {
      appendLog({ timestamp: now(), stream: "system", message: String(error) });
    }
  });

  els.clearLogs.addEventListener("click", () => {
    els.logView.innerHTML = "";
    renderEmptyLog();
  });

  [els.port, els.managementPort, els.extraArgs].forEach((input) => {
    input.addEventListener("input", scheduleSilentSave);
    input.addEventListener("change", scheduleSilentSave);
  });

  els.autoScroll.addEventListener("change", () => {
    autoScroll = els.autoScroll.checked;
    scheduleSilentSave();
  });

  els.fastReconnect.addEventListener("change", scheduleSilentSave);

  els.closeBehaviorSegments.forEach((segment) => {
    segment.addEventListener("click", () => {
      const nextValue = segment.dataset.closeBehavior;
      syncCloseBehaviorUI(nextValue);
      scheduleSilentSave();
      if (nextValue === "tray" && !hasSeenTrayHint()) {
        showTrayHint();
      }
    });
  });

  els.trayHintConfirm.addEventListener("click", () => {
    markTrayHintSeen();
    hideTrayHint();
  });

  window.setInterval(() => {
    refreshStatus().catch((error) => {
      if (runtimeRunning) {
        appendLog({ timestamp: now(), stream: "system", message: String(error) });
      }
    });
  }, 1500);
}

init().catch((error) => {
  appendLog({ timestamp: now(), stream: "system", message: String(error) });
});
