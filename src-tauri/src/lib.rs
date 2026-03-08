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
const TRAY_ID: &str = "main-tray";
const TRAY_SHOW_ID: &str = "tray-show";
const TRAY_EXIT_ID: &str = "tray-exit";

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

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
struct LaunchConfig {
    port: String,
    management_port: String,
    extra_args: String,
    auto_scroll: bool,
    allow_fast_reconnect: bool,
    close_behavior: CloseBehavior,
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
        }
    }
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
struct AppState {
    inner: Mutex<RuntimeState>,
}

#[derive(Default)]
struct RuntimeState {
    child: Option<Child>,
    pid: Option<u32>,
    status: String,
}

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
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(" ; ")
        ));
    }

    let args = build_args(&config)?;
    let mut runtime = state.inner.lock().map_err(|err| err.to_string())?;
    if runtime.child.is_some() {
        return Err("supernode 已在运行".to_owned());
    }

    let mut command = Command::new(&binary_path);
    command
        .args(&args)
        .current_dir(
            binary_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new(".")),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = command.spawn().map_err(|err| err.to_string())?;
    let pid = child.id();

    emit_log(
        &app,
        "system",
        format!("启动命令: {} {}", binary_path.display(), args.join(" ")),
    );

    if let Some(stdout) = child.stdout.take() {
        spawn_reader(app.clone(), stdout, "stdout");
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(app.clone(), stderr, "stderr");
    }

    runtime.pid = Some(pid);
    runtime.status = "运行中".to_owned();
    runtime.child = Some(child);
    drop(runtime);

    save_settings(app.clone(), config)?;
    emit_status(&app, true, "运行中".to_owned(), Some(pid));

    Ok(snapshot(&app, &state)?)
}

#[tauri::command]
fn stop_supernode(app: AppHandle, state: State<AppState>) -> Result<RuntimeSnapshot, String> {
    let mut runtime = state.inner.lock().map_err(|err| err.to_string())?;
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
    let mut runtime = state.inner.lock().map_err(|err| err.to_string())?;
    if let Some(child) = runtime.child.as_mut() {
        match child.try_wait() {
            Ok(Some(status)) => {
                runtime.child = None;
                runtime.pid = None;
                runtime.status = if status.success() {
                    "已停止".to_owned()
                } else {
                    format!("异常退出: {:?}", status.code())
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

fn snapshot(app: &AppHandle, state: &State<AppState>) -> Result<RuntimeSnapshot, String> {
    let runtime = state.inner.lock().map_err(|err| err.to_string())?;
    snapshot_locked(app, &runtime)
}

fn snapshot_locked(app: &AppHandle, runtime: &RuntimeState) -> Result<RuntimeSnapshot, String> {
    Ok(RuntimeSnapshot {
        running: runtime.child.is_some(),
        status: if runtime.status.is_empty() {
            "未启动".to_owned()
        } else {
            runtime.status.clone()
        },
        pid: runtime.pid,
        binary_path: bundled_supernode_path(app)?.display().to_string(),
        source:
            "第三方 Windows 预编译版，随安装程序一起分发，来源：lucktu/n2n（官方 n2n README 提及）"
                .to_owned(),
    })
}

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
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if in_quotes {
        return Err("额外参数里有未闭合的引号".to_owned());
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

fn bundled_supernode_path(app: &AppHandle) -> Result<PathBuf, String> {
    let resource_dir = app.path().resource_dir().map_err(|err| err.to_string())?;
    let candidates = candidate_paths_from_resource_dir(&resource_dir);

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    Ok(candidates.into_iter().next().unwrap_or_else(|| {
        resource_dir
            .join("resources")
            .join("n2n")
            .join("supernode.exe")
    }))
}

fn candidate_supernode_paths(app: &AppHandle) -> Result<Vec<PathBuf>, String> {
    let resource_dir = app.path().resource_dir().map_err(|err| err.to_string())?;
    Ok(candidate_paths_from_resource_dir(&resource_dir))
}

fn candidate_paths_from_resource_dir(resource_dir: &Path) -> Vec<PathBuf> {
    vec![
        resource_dir
            .join("resources")
            .join("n2n")
            .join("supernode.exe"),
        resource_dir.join("n2n").join("supernode.exe"),
        resource_dir.join("supernode.exe"),
    ]
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app.path().app_config_dir().map_err(|err| err.to_string())?;
    fs::create_dir_all(&base).map_err(|err| err.to_string())?;
    Ok(base.join(SETTINGS_FILE))
}

fn save_settings_to_disk(path: &PathBuf, config: &LaunchConfig) -> Result<(), String> {
    let content = serde_json::to_string_pretty(config).map_err(|err| err.to_string())?;
    fs::write(path, content).map_err(|err| err.to_string())
}

fn should_minimize_to_tray(app: &AppHandle) -> bool {
    load_settings(app.clone())
        .map(|config| config.close_behavior == CloseBehavior::Tray)
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
            TRAY_EXIT_ID => app.exit(0),
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

fn emit_log(app: &AppHandle, stream: &str, message: impl Into<String>) {
    let payload = LogPayload {
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        stream: stream.to_owned(),
        message: message.into(),
    };
    let _ = app.emit(LOG_EVENT, payload);
}

fn emit_status(app: &AppHandle, running: bool, status: String, pid: Option<u32>) {
    let payload = StatusPayload {
        running,
        status,
        pid,
    };
    let _ = app.emit(STATUS_EVENT, payload);
}

fn spawn_reader<R>(app: AppHandle, pipe: R, stream: &'static str)
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let reader = BufReader::new(pipe);
        for line in reader.lines() {
            match line {
                Ok(content) => emit_log(&app, stream, content),
                Err(err) => {
                    emit_log(&app, stream, format!("读取日志失败: {err}"));
                    break;
                }
            }
        }
    });
}

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
                if should_minimize_to_tray(&window.app_handle()) {
                    api.prevent_close();
                    let _ = window.hide();
                    emit_log(
                        &window.app_handle(),
                        "system",
                        "已最小化到系统托盘，可通过托盘图标恢复窗口",
                    );
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            start_supernode,
            stop_supernode,
            refresh_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
