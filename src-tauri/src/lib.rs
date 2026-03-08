use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;

use chrono::Local;
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuEvent, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WindowEvent,
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const SETTINGS_FILE: &str = "settings.json";
const LOG_EVENT: &str = "supernode-log";
const STATUS_EVENT: &str = "supernode-status";
const FRPC_LOG_EVENT: &str = "frpc-log";
const FRPC_STATUS_EVENT: &str = "frpc-status";
const TRAY_ID: &str = "main-tray";
const TRAY_SHOW_ID: &str = "tray-show";
const TRAY_EXIT_ID: &str = "tray-exit";

const NATFRP_API_BASE: &str = "https://api.natfrp.com/v4";

// ─────────────────────────────────────────────────────────────
// 数据结构
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum CloseBehavior {
    Exit,
    Tray,
}

impl Default for CloseBehavior {
    fn default() -> Self {
        Self::Exit
    }
}

/// SakuraFRP frpc 相关配置
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
struct FrpcConfig {
    /// 是否随 supernode 启动时联动启动 frpc
    enabled: bool,
    /// 访问密钥 (Token)
    token: String,
    /// 选中的隧道 ID 列表（逗号分隔，由前端基于勾选结果组装）
    tunnel_ids: String,
    /// 自定义 frpc 可执行文件路径（留空则使用内置版）
    custom_path: String,
}

impl Default for FrpcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token: String::new(),
            tunnel_ids: String::new(),
            custom_path: String::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
struct LaunchConfig {
    port: String,
    management_port: String,
    extra_args: String,
    auto_scroll: bool,
    allow_fast_reconnect: bool,
    close_behavior: CloseBehavior,
    frpc: FrpcConfig,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            port: "7654".to_owned(),
            management_port: "5645".to_owned(),
            extra_args: "-f".to_owned(),
            auto_scroll: true,
            allow_fast_reconnect: false,
            close_behavior: CloseBehavior::Exit,
            frpc: FrpcConfig::default(),
        }
    }
}

/// SakuraFRP API 返回的隧道信息（只需要前端展示所需字段）
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TunnelInfo {
    id: u64,
    name: String,
    #[serde(rename = "type")]
    tunnel_type: String,
    node: u64,
    online: bool,
    note: Option<String>,
    remote: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RuntimeSnapshot {
    running: bool,
    status: String,
    pid: Option<u32>,
    binary_path: String,
    source: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FrpcSnapshot {
    running: bool,
    status: String,
    pid: Option<u32>,
    binary_path: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LogPayload {
    timestamp: String,
    stream: String,
    message: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StatusPayload {
    running: bool,
    status: String,
    pid: Option<u32>,
}

#[derive(Default)]
struct RuntimeState {
    child: Option<Child>,
    pid: Option<u32>,
    status: String,
}

#[derive(Default)]
struct FrpcState {
    child: Option<Child>,
    pid: Option<u32>,
    status: String,
}

#[derive(Default)]
struct AppState {
    inner: Mutex<RuntimeState>,
    frpc: Mutex<FrpcState>,
}

// ─────────────────────────────────────────────────────────────
// 退出清理：关闭所有子进程
// ─────────────────────────────────────────────────────────────

fn kill_all_processes(state: &AppState) {
    // 停止 supernode
    if let Ok(mut runtime) = state.inner.lock() {
        if let Some(mut child) = runtime.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        runtime.pid = None;
        runtime.status = "已停止".to_owned();
    }
    // 停止 frpc
    if let Ok(mut frpc) = state.frpc.lock() {
        if let Some(mut child) = frpc.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        frpc.pid = None;
        frpc.status = "已停止".to_owned();
    }
}

// ─────────────────────────────────────────────────────────────
// Tauri 命令：设置
// ─────────────────────────────────────────────────────────────

#[tauri::command]
fn load_settings(app: AppHandle) -> Result<LaunchConfig, String> {
    let path = settings_path(&app)?;
    if !path.exists() {
        let config = LaunchConfig::default();
        save_settings_to_disk(&path, &config)?;
        return Ok(config);
    }
    let content = fs::read_to_string(&path).map_err(|err| err.to_string())?;
    serde_json::from_str(&content).map_err(|err| err.to_string())
}

#[tauri::command]
fn save_settings(app: AppHandle, config: LaunchConfig) -> Result<(), String> {
    let path = settings_path(&app)?;
    save_settings_to_disk(&path, &config)
}

// ─────────────────────────────────────────────────────────────
// Tauri 命令：supernode
// ─────────────────────────────────────────────────────────────

#[tauri::command]
fn start_supernode(
    app: AppHandle,
    state: State<AppState>,
    config: LaunchConfig,
) -> Result<RuntimeSnapshot, String> {
    let binary_path = bundled_supernode_path(&app)?;
    if !binary_path.exists() {
        return Err(format!(
            "未找到内置 supernode.exe，已检查路径: {}",
            candidate_supernode_paths(&app)?
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(" ; ")
        ));
    }

    let args = build_args(&config)?;
    let mut runtime = state.inner.lock().map_err(|e| e.to_string())?;
    if runtime.child.is_some() {
        return Err("supernode 已在运行".to_owned());
    }

    let mut command = Command::new(&binary_path);
    command
        .args(&args)
        .current_dir(binary_path.parent().unwrap_or_else(|| Path::new(".")))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let pid = child.id();

    emit_log(&app, "system", format!("启动命令: {} {}", binary_path.display(), args.join(" ")));

    if let Some(stdout) = child.stdout.take() {
        spawn_reader(app.clone(), stdout, "stdout", LOG_EVENT);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(app.clone(), stderr, "stderr", LOG_EVENT);
    }

    runtime.pid = Some(pid);
    runtime.status = "运行中".to_owned();
    runtime.child = Some(child);
    drop(runtime);

    // 联动启动 frpc
    if config.frpc.enabled {
        let _ = start_frpc_internal(&app, &state, &config.frpc);
    }

    save_settings(app.clone(), config)?;
    emit_status(&app, true, "运行中".to_owned(), Some(pid));
    Ok(snapshot(&app, &state)?)
}

#[tauri::command]
fn stop_supernode(app: AppHandle, state: State<AppState>) -> Result<RuntimeSnapshot, String> {
    let mut runtime = state.inner.lock().map_err(|e| e.to_string())?;
    let Some(mut child) = runtime.child.take() else {
        runtime.status = "未启动".to_owned();
        return snapshot_locked(&app, &runtime);
    };

    let pid = runtime.pid.take();
    match child.kill() {
        Ok(()) => {
            let _ = child.wait();
            runtime.status = "已停止".to_owned();
            emit_log(&app, "system", "已发送停止信号");
            emit_status(&app, false, "已停止".to_owned(), None);
        }
        Err(err) => {
            runtime.status = format!("停止失败: {err}");
            emit_log(&app, "system", format!("停止失败: {err}"));
            emit_status(&app, false, runtime.status.clone(), pid);
        }
    }
    snapshot_locked(&app, &runtime)
}

#[tauri::command]
fn refresh_status(app: AppHandle, state: State<AppState>) -> Result<RuntimeSnapshot, String> {
    let mut runtime = state.inner.lock().map_err(|e| e.to_string())?;
    if let Some(child) = runtime.child.as_mut() {
        match child.try_wait() {
            Ok(Some(s)) => {
                runtime.child = None;
                runtime.pid = None;
                runtime.status = if s.success() {
                    "已停止".to_owned()
                } else {
                    format!("异常退出: {:?}", s.code())
                };
                emit_log(&app, "system", runtime.status.clone());
                emit_status(&app, false, runtime.status.clone(), None);
            }
            Ok(None) => {
                if runtime.status.is_empty() {
                    runtime.status = "运行中".to_owned();
                }
            }
            Err(err) => {
                runtime.child = None;
                runtime.pid = None;
                runtime.status = format!("状态检查失败: {err}");
                emit_log(&app, "system", runtime.status.clone());
                emit_status(&app, false, runtime.status.clone(), None);
            }
        }
    } else if runtime.status.is_empty() {
        runtime.status = "未启动".to_owned();
    }
    snapshot_locked(&app, &runtime)
}

// ─────────────────────────────────────────────────────────────
// Tauri 命令：frpc
// ─────────────────────────────────────────────────────────────

fn start_frpc_internal(
    app: &AppHandle,
    state: &State<AppState>,
    frpc_config: &FrpcConfig,
) -> Result<FrpcSnapshot, String> {
    if frpc_config.token.trim().is_empty() {
        return Err("访问密钥不能为空".to_owned());
    }
    if frpc_config.tunnel_ids.trim().is_empty() {
        return Err("未选择任何隧道".to_owned());
    }

    let binary_path = resolve_frpc_path(app, frpc_config)?;
    if !binary_path.exists() {
        return Err(format!("未找到 frpc 可执行文件: {}", binary_path.display()));
    }

    let mut frpc = state.frpc.lock().map_err(|e| e.to_string())?;
    if frpc.child.is_some() {
        return Err("frpc 已在运行".to_owned());
    }

    let fetch_arg = format!("{}:{}", frpc_config.token.trim(), frpc_config.tunnel_ids.trim());

    let mut command = Command::new(&binary_path);
    command
        .args(["-f", &fetch_arg])
        .current_dir(binary_path.parent().unwrap_or_else(|| Path::new(".")))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let pid = child.id();

    // 日志中隐藏 token，仅显示隧道 ID
    emit_frpc_log(app, "system", format!("启动 frpc，隧道: {}", frpc_config.tunnel_ids.trim()));

    if let Some(stdout) = child.stdout.take() {
        spawn_reader(app.clone(), stdout, "stdout", FRPC_LOG_EVENT);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(app.clone(), stderr, "stderr", FRPC_LOG_EVENT);
    }

    frpc.pid = Some(pid);
    frpc.status = "运行中".to_owned();
    frpc.child = Some(child);

    emit_frpc_status(app, true, "运行中".to_owned(), Some(pid));
    Ok(frpc_snapshot_locked(app, &frpc)?)
}

#[tauri::command]
fn start_frpc(
    app: AppHandle,
    state: State<AppState>,
    config: LaunchConfig,
) -> Result<FrpcSnapshot, String> {
    save_settings(app.clone(), config.clone())?;
    start_frpc_internal(&app, &state, &config.frpc)
}

#[tauri::command]
fn stop_frpc(app: AppHandle, state: State<AppState>) -> Result<FrpcSnapshot, String> {
    let mut frpc = state.frpc.lock().map_err(|e| e.to_string())?;
    let Some(mut child) = frpc.child.take() else {
        frpc.status = "未启动".to_owned();
        return frpc_snapshot_locked(&app, &frpc);
    };

    let pid = frpc.pid.take();
    match child.kill() {
        Ok(()) => {
            let _ = child.wait();
            frpc.status = "已停止".to_owned();
            emit_frpc_log(&app, "system", "已发送停止信号");
            emit_frpc_status(&app, false, "已停止".to_owned(), None);
        }
        Err(err) => {
            frpc.status = format!("停止失败: {err}");
            emit_frpc_log(&app, "system", format!("停止失败: {err}"));
            emit_frpc_status(&app, false, frpc.status.clone(), pid);
        }
    }
    frpc_snapshot_locked(&app, &frpc)
}

#[tauri::command]
fn refresh_frpc_status(app: AppHandle, state: State<AppState>) -> Result<FrpcSnapshot, String> {
    let mut frpc = state.frpc.lock().map_err(|e| e.to_string())?;
    if let Some(child) = frpc.child.as_mut() {
        match child.try_wait() {
            Ok(Some(s)) => {
                frpc.child = None;
                frpc.pid = None;
                frpc.status = if s.success() {
                    "已停止".to_owned()
                } else {
                    format!("异常退出: {:?}", s.code())
                };
                emit_frpc_log(&app, "system", frpc.status.clone());
                emit_frpc_status(&app, false, frpc.status.clone(), None);
            }
            Ok(None) => {
                if frpc.status.is_empty() {
                    frpc.status = "运行中".to_owned();
                }
            }
            Err(err) => {
                frpc.child = None;
                frpc.pid = None;
                frpc.status = format!("状态检查失败: {err}");
                emit_frpc_log(&app, "system", frpc.status.clone());
                emit_frpc_status(&app, false, frpc.status.clone(), None);
            }
        }
    } else if frpc.status.is_empty() {
        frpc.status = "未启动".to_owned();
    }
    frpc_snapshot_locked(&app, &frpc)
}

/// 通过 SakuraFRP API 获取隧道列表
#[tauri::command]
async fn fetch_tunnels(token: String) -> Result<Vec<TunnelInfo>, String> {
    if token.trim().is_empty() {
        return Err("请先输入访问密钥".to_owned());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;

    let url = format!("{NATFRP_API_BASE}/tunnels");
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token.trim()))
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        // 尝试解析错误消息
        let body = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["msg"].as_str().map(|s| s.to_owned()))
            .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
        return Err(format!("API 错误: {msg}"));
    }

    let tunnels: Vec<TunnelInfo> = resp
        .json()
        .await
        .map_err(|e| format!("解析响应失败: {e}"))?;

    Ok(tunnels)
}

// ─────────────────────────────────────────────────────────────
// Snapshot 辅助
// ─────────────────────────────────────────────────────────────

fn snapshot(app: &AppHandle, state: &State<AppState>) -> Result<RuntimeSnapshot, String> {
    let runtime = state.inner.lock().map_err(|e| e.to_string())?;
    snapshot_locked(app, &runtime)
}

fn snapshot_locked(app: &AppHandle, runtime: &RuntimeState) -> Result<RuntimeSnapshot, String> {
    Ok(RuntimeSnapshot {
        running: runtime.child.is_some(),
        status: if runtime.status.is_empty() { "未启动".to_owned() } else { runtime.status.clone() },
        pid: runtime.pid,
        binary_path: bundled_supernode_path(app)?.display().to_string(),
        source: "第三方 Windows 预编译版，随安装程序一起分发，来源：lucktu/n2n（官方 n2n README 提及）".to_owned(),
    })
}

fn frpc_snapshot_locked(app: &AppHandle, frpc: &FrpcState) -> Result<FrpcSnapshot, String> {
    let binary_path = bundled_frpc_path(app)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "未知".to_owned());
    Ok(FrpcSnapshot {
        running: frpc.child.is_some(),
        status: if frpc.status.is_empty() { "未启动".to_owned() } else { frpc.status.clone() },
        pid: frpc.pid,
        binary_path,
    })
}

// ─────────────────────────────────────────────────────────────
// 参数构建
// ─────────────────────────────────────────────────────────────

fn build_args(config: &LaunchConfig) -> Result<Vec<String>, String> {
    let port = config.port.trim();
    if port.is_empty() {
        return Err("监听端口不能为空".to_owned());
    }

    let mut args = vec!["-p".to_owned(), port.to_owned()];
    let management_port = config.management_port.trim();
    if !management_port.is_empty() {
        args.push("-t".to_owned());
        args.push(management_port.to_owned());
    }
    if config.allow_fast_reconnect {
        args.push("-M".to_owned());
    }
    args.extend(split_args(config.extra_args.trim())?);
    Ok(args)
}

fn split_args(input: &str) -> Result<Vec<String>, String> {
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in input.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() { args.push(std::mem::take(&mut current)); }
            }
            _ => current.push(ch),
        }
    }

    if in_quotes { return Err("额外参数里有未闭合的引号".to_owned()); }
    if !current.is_empty() { args.push(current); }
    Ok(args)
}

// ─────────────────────────────────────────────────────────────
// 路径辅助
// ─────────────────────────────────────────────────────────────

fn bundled_supernode_path(app: &AppHandle) -> Result<PathBuf, String> {
    let resource_dir = app.path().resource_dir().map_err(|e| e.to_string())?;
    let candidates = candidate_paths_from_resource_dir(&resource_dir);
    for candidate in &candidates {
        if candidate.exists() { return Ok(candidate.clone()); }
    }
    Ok(candidates.into_iter().next().unwrap_or_else(|| resource_dir.join("resources").join("n2n").join("supernode.exe")))
}

fn candidate_supernode_paths(app: &AppHandle) -> Result<Vec<PathBuf>, String> {
    let resource_dir = app.path().resource_dir().map_err(|e| e.to_string())?;
    Ok(candidate_paths_from_resource_dir(&resource_dir))
}

fn candidate_paths_from_resource_dir(resource_dir: &Path) -> Vec<PathBuf> {
    vec![
        resource_dir.join("resources").join("n2n").join("supernode.exe"),
        resource_dir.join("n2n").join("supernode.exe"),
        resource_dir.join("supernode.exe"),
    ]
}

fn bundled_frpc_path(app: &AppHandle) -> Result<PathBuf, String> {
    let resource_dir = app.path().resource_dir().map_err(|e| e.to_string())?;
    let candidates = vec![
        resource_dir.join("resources").join("frpc").join("frpc_windows_amd64.exe"),
        resource_dir.join("resources").join("frpc").join("frpc.exe"),
        resource_dir.join("frpc").join("frpc_windows_amd64.exe"),
        resource_dir.join("frpc").join("frpc.exe"),
        resource_dir.join("frpc_windows_amd64.exe"),
        resource_dir.join("frpc.exe"),
    ];
    for candidate in &candidates {
        if candidate.exists() { return Ok(candidate.clone()); }
    }
    Ok(candidates.into_iter().next().unwrap())
}

fn resolve_frpc_path(app: &AppHandle, config: &FrpcConfig) -> Result<PathBuf, String> {
    let custom = config.custom_path.trim();
    if !custom.is_empty() { return Ok(PathBuf::from(custom)); }
    bundled_frpc_path(app)
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    Ok(base.join(SETTINGS_FILE))
}

fn save_settings_to_disk(path: &PathBuf, config: &LaunchConfig) -> Result<(), String> {
    let content = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}

// ─────────────────────────────────────────────────────────────
// 托盘
// ─────────────────────────────────────────────────────────────

fn should_minimize_to_tray(app: &AppHandle) -> bool {
    load_settings(app.clone())
        .map(|c| c.close_behavior == CloseBehavior::Tray)
        .unwrap_or(false)
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show_item = MenuItemBuilder::with_id(TRAY_SHOW_ID, "显示主窗口").build(app)?;
    let exit_item = MenuItemBuilder::with_id(TRAY_EXIT_ID, "退出软件").build(app)?;
    let menu = Menu::with_items(app, &[&show_item, &exit_item])?;
    let app_for_tray = app.clone();

    let tray_builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("n2n Supernode Launcher")
        .on_menu_event(|app, event: MenuEvent| match event.id().as_ref() {
            TRAY_SHOW_ID => show_main_window(app),
            TRAY_EXIT_ID => {
                // 退出前关闭所有服务
                if let Some(state) = app.try_state::<AppState>() {
                    kill_all_processes(&state);
                }
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(move |_tray, event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(&app_for_tray);
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        tray_builder.icon(icon).build(app)?;
    } else {
        tray_builder.build(app)?;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────
// 事件发射
// ─────────────────────────────────────────────────────────────

fn emit_log(app: &AppHandle, stream: &str, message: impl Into<String>) {
    let _ = app.emit(LOG_EVENT, LogPayload {
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        stream: stream.to_owned(),
        message: message.into(),
    });
}

fn emit_status(app: &AppHandle, running: bool, status: String, pid: Option<u32>) {
    let _ = app.emit(STATUS_EVENT, StatusPayload { running, status, pid });
}

fn emit_frpc_log(app: &AppHandle, stream: &str, message: impl Into<String>) {
    let _ = app.emit(FRPC_LOG_EVENT, LogPayload {
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        stream: stream.to_owned(),
        message: message.into(),
    });
}

fn emit_frpc_status(app: &AppHandle, running: bool, status: String, pid: Option<u32>) {
    let _ = app.emit(FRPC_STATUS_EVENT, StatusPayload { running, status, pid });
}

fn spawn_reader<R>(app: AppHandle, pipe: R, stream: &'static str, event: &'static str)
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let reader = BufReader::new(pipe);
        for line in reader.lines() {
            match line {
                Ok(content) => {
                    let _ = app.emit(event, LogPayload {
                        timestamp: Local::now().format("%H:%M:%S").to_string(),
                        stream: stream.to_owned(),
                        message: content,
                    });
                }
                Err(err) => {
                    let _ = app.emit(event, LogPayload {
                        timestamp: Local::now().format("%H:%M:%S").to_string(),
                        stream: stream.to_owned(),
                        message: format!("读取日志失败: {err}"),
                    });
                    break;
                }
            }
        }
    });
}

// ─────────────────────────────────────────────────────────────
// 入口
// ─────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|app| {
            build_tray(app.handle())?;
            Ok(())
        })
        .plugin(tauri_plugin_log::Builder::default().build())
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }

            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                if should_minimize_to_tray(app) {
                    // 最小化到托盘，不关服务
                    api.prevent_close();
                    let _ = window.hide();
                    emit_log(app, "system", "已最小化到系统托盘，可通过托盘图标恢复窗口");
                } else {
                    // 直接退出 → 关闭所有子进程
                    if let Some(state) = app.try_state::<AppState>() {
                        kill_all_processes(&state);
                    }
                    // 让窗口正常关闭（不阻止）
                }
            }

            if let WindowEvent::Destroyed = event {
                // 窗口销毁时再次确保进程已清理（防止意外情况）
                let app = window.app_handle();
                if let Some(state) = app.try_state::<AppState>() {
                    kill_all_processes(&state);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            start_supernode,
            stop_supernode,
            refresh_status,
            start_frpc,
            stop_frpc,
            refresh_frpc_status,
            fetch_tunnels,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
