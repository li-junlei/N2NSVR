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
  // frpc elements
  frpcToken: document.querySelector("#frpcTokenInput"),
  frpcFetchBtn: document.querySelector("#frpcFetchTunnelsButton"),
  frpcTunnelList: document.querySelector("#frpcTunnelList"),
  frpcEnabled: document.querySelector("#frpcEnabledInput"),
  frpcCustomPath: document.querySelector("#frpcCustomPathInput"),
  frpcStart: document.querySelector("#frpcStartButton"),
  frpcStop: document.querySelector("#frpcStopButton"),
  frpcSave: document.querySelector("#frpcSaveButton"),
  frpcClearLogs: document.querySelector("#frpcClearLogsButton"),
  frpcStatusText: document.querySelector("#frpcStatusText"),
  frpcPidText: document.querySelector("#frpcPidText"),
  frpcLogView: document.querySelector("#frpcLogView"),
};

let autoScroll = true;
let runtimeRunning = false;
let frpcRunning = false;
let closeBehavior = "exit";
let saveTimer = null;

// 已勾选的隧道 ID 集合（持久化到 settings）
let selectedTunnelIds = new Set();
// 上次拉取的隧道列表（用于重绘勾选状态）
let cachedTunnels = [];

// ─── 标签页 ───────────────────────────────────────────────────

function selectTab(tabName) {
  const pageMap = { home: "home", frpc: "frpc", settings: "settings" };
  const pageName = pageMap[tabName] ?? "home";
  els.tabs.forEach((tab) => tab.classList.toggle("active", tab.dataset.tab === tabName));
  els.pages.forEach((page) => page.classList.toggle("active", page.dataset.page === pageName));
}

// ─── 托盘提示 ────────────────────────────────────────────────────

function hasSeenTrayHint() { return window.localStorage.getItem(TRAY_HINT_SEEN_KEY) === "1"; }
function markTrayHintSeen() { window.localStorage.setItem(TRAY_HINT_SEEN_KEY, "1"); }
function showTrayHint() {
  els.trayHintModal.classList.remove("hidden");
  els.trayHintModal.setAttribute("aria-hidden", "false");
}
function hideTrayHint() {
  els.trayHintModal.classList.add("hidden");
  els.trayHintModal.setAttribute("aria-hidden", "true");
}

// ─── 日志 ────────────────────────────────────────────────────────

function renderEmptyLog(logView) {
  if (logView.childElementCount === 0) {
    const empty = document.createElement("div");
    empty.className = "log-empty";
    empty.textContent = "请启动或重新连接以显示日志";
    logView.append(empty);
  }
}

function clearEmptyLog(logView) {
  const empty = logView.querySelector(".log-empty");
  if (empty) empty.remove();
}

function appendLog(entry, logView) {
  clearEmptyLog(logView);
  const row = document.createElement("div");
  row.className = `log-line ${entry.stream || "stdout"}`;
  row.innerHTML = `
    <span class="time">${entry.timestamp || "--:--:--"}</span>
    <span class="stream">${entry.stream || "stdout"}</span>
    <span class="message"></span>
  `;
  row.querySelector(".message").textContent = entry.message ?? "";
  logView.append(row);
  if (autoScroll) logView.scrollTop = logView.scrollHeight;
}

// ─── 状态 ─────────────────────────────────────────────────────

function setRunningState(running, status, pid) {
  runtimeRunning = running;
  els.statusText.textContent = status || (running ? "运行中" : "未启动");
  els.pidText.textContent = `PID: ${pid ?? "-"}`;
  els.start.disabled = running;
  els.stop.disabled = !running;
  els.statusText.classList.toggle("running", running);
}

function setFrpcRunningState(running, status, pid) {
  frpcRunning = running;
  els.frpcStatusText.textContent = status || (running ? "运行中" : "未启动");
  els.frpcPidText.textContent = `PID: ${pid ?? "-"}`;
  els.frpcStart.disabled = running;
  els.frpcStop.disabled = !running;
  els.frpcStatusText.classList.toggle("running", running);
}

function syncCloseBehaviorUI(value) {
  closeBehavior = value;
  els.closeBehaviorSegments.forEach((seg) =>
    seg.classList.toggle("active", seg.dataset.closeBehavior === value)
  );
}

// ─── 隧道列表渲染 ────────────────────────────────────────────────

function tunnelTypeLabel(type) {
  const map = { tcp: "TCP", udp: "UDP", http: "HTTP", https: "HTTPS", wol: "WoL", etcp: "ETCP", eudp: "EUDP" };
  return map[type] ?? type.toUpperCase();
}

function renderTunnelList(tunnels) {
  if (!tunnels || tunnels.length === 0) {
    els.frpcTunnelList.innerHTML = `<div class="tunnel-empty">该账号下暂无隧道</div>`;
    els.frpcTunnelList.classList.remove("hidden");
    return;
  }

  els.frpcTunnelList.innerHTML = "";
  tunnels.forEach((t) => {
    const id = String(t.id);
    const checked = selectedTunnelIds.has(id);
    const item = document.createElement("label");
    item.className = "tunnel-item";
    item.innerHTML = `
      <input type="checkbox" class="tunnel-check" data-id="${id}" ${checked ? "checked" : ""} />
      <span class="tunnel-badge type-${t.tunnelType ?? t.type}">${tunnelTypeLabel(t.tunnelType ?? t.type)}</span>
      <span class="tunnel-name">${t.name}</span>
      <span class="tunnel-meta">#${id}${t.remote ? " · " + t.remote : ""}${t.note ? " · " + t.note : ""}</span>
      <span class="tunnel-dot ${t.online ? "online" : "offline"}"></span>
    `;
    els.frpcTunnelList.append(item);
  });

  els.frpcTunnelList.classList.remove("hidden");

  // 监听勾选变化
  els.frpcTunnelList.querySelectorAll(".tunnel-check").forEach((checkbox) => {
    checkbox.addEventListener("change", () => {
      if (checkbox.checked) {
        selectedTunnelIds.add(checkbox.dataset.id);
      } else {
        selectedTunnelIds.delete(checkbox.dataset.id);
      }
      scheduleSilentSave();
    });
  });
}

// ─── 表单 读写 ─────────────────────────────────────────────────

/** 从 selectedTunnelIds 集合中构建逗号分隔的 ID 字符串 */
function getSelectedTunnelIdsStr() {
  return Array.from(selectedTunnelIds).join(",");
}

function readForm() {
  return {
    port: els.port.value.trim(),
    managementPort: els.managementPort.value.trim(),
    extraArgs: els.extraArgs.value.trim(),
    autoScroll: els.autoScroll.checked,
    allowFastReconnect: els.fastReconnect.checked,
    closeBehavior,
    frpc: {
      enabled: els.frpcEnabled.checked,
      token: els.frpcToken.value.trim(),
      tunnelIds: getSelectedTunnelIdsStr(),
      customPath: els.frpcCustomPath.value.trim(),
    },
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

  const frpc = config.frpc ?? {};
  els.frpcEnabled.checked = frpc.enabled ?? false;
  els.frpcToken.value = frpc.token ?? "";
  els.frpcCustomPath.value = frpc.customPath ?? "";

  // 恢复已选隧道 ID
  selectedTunnelIds = new Set(
    (frpc.tunnelIds ?? "").split(",").map((s) => s.trim()).filter(Boolean)
  );
}

// ─── 保存 ─────────────────────────────────────────────────────

async function saveSettings({ silent = false } = {}) {
  const config = readForm();
  await invoke("save_settings", { config });
  if (!silent) {
    appendLog({ timestamp: now(), stream: "system", message: "配置已保存" }, els.logView);
  }
}

function scheduleSilentSave() {
  window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    saveSettings({ silent: true }).catch((error) => {
      appendLog({ timestamp: now(), stream: "system", message: `保存配置失败: ${String(error)}` }, els.logView);
    });
  }, 300);
}

// ─── Supernode 操作 ───────────────────────────────────────────

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

// ─── frpc 操作 ────────────────────────────────────────────────

async function refreshFrpcStatus() {
  const snapshot = await invoke("refresh_frpc_status");
  setFrpcRunningState(snapshot.running, snapshot.status, snapshot.pid);
}

async function startFrpc() {
  const config = readForm();
  if (!getSelectedTunnelIdsStr()) {
    appendLog({ timestamp: now(), stream: "system", message: "请先获取隧道列表并至少勾选一条隧道" }, els.frpcLogView);
    return;
  }
  const snapshot = await invoke("start_frpc", { config });
  setFrpcRunningState(snapshot.running, snapshot.status, snapshot.pid);
}

async function stopFrpc() {
  const snapshot = await invoke("stop_frpc");
  setFrpcRunningState(snapshot.running, snapshot.status, snapshot.pid);
}

async function fetchTunnels() {
  const token = els.frpcToken.value.trim();
  if (!token) {
    appendLog({ timestamp: now(), stream: "system", message: "请先输入访问密钥再获取隧道列表" }, els.frpcLogView);
    return;
  }

  els.frpcFetchBtn.disabled = true;
  els.frpcFetchBtn.textContent = "获取中…";

  try {
    const tunnels = await invoke("fetch_tunnels", { token });
    cachedTunnels = tunnels;
    renderTunnelList(tunnels);
    appendLog({
      timestamp: now(),
      stream: "system",
      message: `已获取 ${tunnels.length} 条隧道`,
    }, els.frpcLogView);
    scheduleSilentSave();
  } catch (error) {
    appendLog({ timestamp: now(), stream: "system", message: `获取隧道列表失败: ${String(error)}` }, els.frpcLogView);
  } finally {
    els.frpcFetchBtn.disabled = false;
    els.frpcFetchBtn.textContent = "重新获取";
  }
}

// ─── 工具 ─────────────────────────────────────────────────────

function now() {
  return new Date().toLocaleTimeString("zh-CN", { hour12: false });
}

// ─── 初始化 ───────────────────────────────────────────────────

async function init() {
  if (!invoke || !listen) {
    appendLog({
      timestamp: now(),
      stream: "system",
      message: "未检测到 Tauri API，当前页面需要通过 Tauri 应用运行。",
    }, els.logView);
    setRunningState(false, "无法连接 Tauri", null);
    return;
  }

  renderEmptyLog(els.logView);
  renderEmptyLog(els.frpcLogView);
  selectTab("home");
  applyForm(await invoke("load_settings"));
  await refreshStatus();
  await refreshFrpcStatus();

  // 若存有已选 ID，显示恢复提示
  if (selectedTunnelIds.size > 0) {
    els.frpcTunnelList.innerHTML = `<div class="tunnel-empty">已保存 ${selectedTunnelIds.size} 条隧道选择，点击「重新获取」刷新列表</div>`;
    els.frpcTunnelList.classList.remove("hidden");
    els.frpcFetchBtn.textContent = "重新获取";
  }

  // 事件监听
  await listen("supernode-log", (event) => appendLog(event.payload, els.logView));
  await listen("supernode-status", (event) => {
    const { running, status, pid } = event.payload;
    setRunningState(running, status, pid);
  });

  await listen("frpc-log", (event) => appendLog(event.payload, els.frpcLogView));
  await listen("frpc-status", (event) => {
    const { running, status, pid } = event.payload;
    setFrpcRunningState(running, status, pid);
  });

  // 标签页
  els.tabs.forEach((tab) => tab.addEventListener("click", () => selectTab(tab.dataset.tab)));

  // Supernode 控制
  els.start.addEventListener("click", async () => {
    try {
      await startSupernode();
      els.homeLogCard?.scrollIntoView({ behavior: "smooth", block: "start" });
    } catch (error) {
      appendLog({ timestamp: now(), stream: "system", message: String(error) }, els.logView);
    }
  });

  els.stop.addEventListener("click", async () => {
    try { await stopSupernode(); }
    catch (error) { appendLog({ timestamp: now(), stream: "system", message: String(error) }, els.logView); }
  });

  els.save.addEventListener("click", async () => {
    try { await saveSettings(); }
    catch (error) { appendLog({ timestamp: now(), stream: "system", message: String(error) }, els.logView); }
  });

  els.clearLogs.addEventListener("click", () => {
    els.logView.innerHTML = "";
    renderEmptyLog(els.logView);
  });

  // frpc 控制
  els.frpcFetchBtn.addEventListener("click", () => fetchTunnels().catch(console.error));

  els.frpcStart.addEventListener("click", async () => {
    try { await startFrpc(); }
    catch (error) { appendLog({ timestamp: now(), stream: "system", message: String(error) }, els.frpcLogView); }
  });

  els.frpcStop.addEventListener("click", async () => {
    try { await stopFrpc(); }
    catch (error) { appendLog({ timestamp: now(), stream: "system", message: String(error) }, els.frpcLogView); }
  });

  els.frpcSave.addEventListener("click", async () => {
    try {
      const config = readForm();
      await invoke("save_settings", { config });
      appendLog({ timestamp: now(), stream: "system", message: "frpc 配置已保存" }, els.frpcLogView);
    } catch (error) {
      appendLog({ timestamp: now(), stream: "system", message: String(error) }, els.frpcLogView);
    }
  });

  els.frpcClearLogs.addEventListener("click", () => {
    els.frpcLogView.innerHTML = "";
    renderEmptyLog(els.frpcLogView);
  });

  // 输入联动保存
  [els.port, els.managementPort, els.extraArgs, els.frpcCustomPath].forEach((input) => {
    input.addEventListener("input", scheduleSilentSave);
    input.addEventListener("change", scheduleSilentSave);
  });

  // Token 变化时重置隧道列表
  els.frpcToken.addEventListener("change", () => {
    if (cachedTunnels.length > 0) {
      els.frpcTunnelList.innerHTML = `<div class="tunnel-empty">Token 已变更，请重新获取隧道列表</div>`;
      cachedTunnels = [];
    }
    scheduleSilentSave();
  });
  els.frpcToken.addEventListener("input", scheduleSilentSave);

  els.autoScroll.addEventListener("change", () => {
    autoScroll = els.autoScroll.checked;
    scheduleSilentSave();
  });

  els.fastReconnect.addEventListener("change", scheduleSilentSave);
  els.frpcEnabled.addEventListener("change", scheduleSilentSave);

  els.closeBehaviorSegments.forEach((segment) => {
    segment.addEventListener("click", () => {
      const nextValue = segment.dataset.closeBehavior;
      syncCloseBehaviorUI(nextValue);
      scheduleSilentSave();
      if (nextValue === "tray" && !hasSeenTrayHint()) showTrayHint();
    });
  });

  els.trayHintConfirm.addEventListener("click", () => {
    markTrayHintSeen();
    hideTrayHint();
  });

  // 定时轮询
  window.setInterval(() => {
    refreshStatus().catch((error) => {
      if (runtimeRunning) appendLog({ timestamp: now(), stream: "system", message: String(error) }, els.logView);
    });
    refreshFrpcStatus().catch((error) => {
      if (frpcRunning) appendLog({ timestamp: now(), stream: "system", message: String(error) }, els.frpcLogView);
    });
  }, 1500);
}

init().catch((error) => {
  appendLog({ timestamp: now(), stream: "system", message: String(error) }, els.logView);
});
