use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread,
};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
struct InstallerState {
    running: Arc<AtomicBool>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct InstallerFinished {
    success: bool,
    exit_code: i32,
}

fn project_root() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| "Could not resolve updater project root.".to_string())
}

fn emit_output(app: &AppHandle, text: String) {
    let _ = app.emit("installer-output", serde_json::json!({ "text": text }));
}

#[tauri::command]
fn start_install(
    app: AppHandle,
    state: State<'_, InstallerState>,
    action: String,
) -> Result<(), String> {
    let script_name = match action.as_str() {
        "winget" => "Install-Winget.ps1",
        "store" => "Install-Store.ps1",
        _ => return Err("Unsupported installation action.".to_string()),
    };

    let (base_directory, node_executable, host_script) = if cfg!(debug_assertions) {
        let root = project_root()?;
        (
            root.clone(),
            PathBuf::from("node.exe"),
            root.join("scripts").join("pty-host.cjs"),
        )
    } else {
        let resources = app
            .path()
            .resource_dir()
            .map_err(|error| format!("Could not find application resources: {error}"))?
            .join("resources")
            .join("generated")
            .join("updater");
        (
            resources.clone(),
            resources.join("node.exe"),
            resources.join("pty-host.cjs"),
        )
    };

    if !host_script.is_file() {
        return Err(format!("PTY host script not found: {}", host_script.display()));
    }
    let installer_script = base_directory.join("scripts").join(script_name);
    if !installer_script.is_file() {
        return Err(format!("Installer script not found: {}", installer_script.display()));
    }
    if !cfg!(debug_assertions) && !node_executable.is_file() {
        return Err(format!("Bundled Node runtime not found: {}", node_executable.display()));
    }

    if state
        .running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("Another installation is already running.".to_string());
    }

    let mut child = match Command::new(node_executable)
        .arg(host_script)
        .arg(&action)
        .current_dir(base_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            state.running.store(false, Ordering::Release);
            return Err(format!("Could not start the bundled installer host: {error}"));
        }
    };

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            state.running.store(false, Ordering::Release);
            return Err("Installer host did not provide an output stream.".to_string());
        }
    };
    let stderr = child.stderr.take();
    let app_handle = app.clone();
    let running = Arc::clone(&state.running);

    thread::spawn(move || {
        let stderr_thread = stderr.map(|stderr| {
            let stderr_app = app_handle.clone();
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    emit_output(&stderr_app, format!("{line}\n"));
                }
            })
        });

        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            match serde_json::from_str::<Value>(&line) {
                Ok(message) if message["type"] == "output" => {
                    if let Some(text) = message["data"].as_str() {
                        emit_output(&app_handle, text.to_string());
                    }
                }
                Ok(_) => {}
                Err(_) => emit_output(&app_handle, format!("{line}\n")),
            }
        }

        if let Some(stderr_thread) = stderr_thread {
            let _ = stderr_thread.join();
        }
        let exit_code = child
            .wait()
            .map(|status| status.code().unwrap_or(1))
            .unwrap_or(1);
        running.store(false, Ordering::Release);
        let _ = app_handle.emit(
            "installer-finished",
            InstallerFinished {
                success: exit_code == 0,
                exit_code,
            },
        );
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(InstallerState::default())
        .invoke_handler(tauri::generate_handler![start_install])
        .run(tauri::generate_context!())
        .expect("error while running Omniscience Updater");
}
