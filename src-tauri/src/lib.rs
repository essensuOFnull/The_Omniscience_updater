use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri::{Url, WebviewUrl, WebviewWindowBuilder};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagerState {
    mode: String,
    source_directory: String,
    production_executable: String,
    #[serde(default = "default_drive_letter")]
    drive_letter: String,
    #[serde(default)]
    browser_url: String,
}

fn default_drive_letter() -> String {
    "C".to_string()
}

impl Default for ManagerState {
    fn default() -> Self {
        let source_directory = project_root()
            .ok()
            .and_then(|root| root.parent().map(PathBuf::from))
            .map(|parent| parent.join("The_Omniscience").to_string_lossy().to_string())
            .unwrap_or_default();

        Self {
            mode: "prod".to_string(),
            source_directory,
            production_executable: String::new(),
            drive_letter: default_drive_letter(),
            browser_url: "https://github.com/EssenceOfDream/The_Omniscience/releases".to_string(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceStatus {
    exists: bool,
    is_git_repo: bool,
    branch: Option<String>,
    commit: Option<String>,
    tag: Option<String>,
    dirty_tracked: bool,
    status: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseStatus {
    available: bool,
    tag: Option<String>,
    name: Option<String>,
    published_at: Option<String>,
    url: Option<String>,
    error: Option<String>,
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

fn state_file(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app.path().app_data_dir()
        .map_err(|error| format!("Could not resolve application data directory: {error}"))?;
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create application data directory: {error}"))?;
    Ok(directory.join("manager-state.json"))
}

fn load_state(app: &AppHandle) -> ManagerState {
    let path = match state_file(app) {
        Ok(path) => path,
        Err(_) => return ManagerState::default(),
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_state(app: &AppHandle, state: &ManagerState) -> Result<(), String> {
    let path = state_file(app)?;
    let text = serde_json::to_string_pretty(state)
        .map_err(|error| format!("Could not serialize manager state: {error}"))?;
    fs::write(path, text)
        .map_err(|error| format!("Could not save manager state: {error}"))
}

fn sanitize_path_component(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Path component is empty.".to_string());
    }
    if value == "." || value == ".." || value.contains(['/', '\\', ':']) {
        return Err(format!("Недопустимый компонент пути: {value}"));
    }
    let sanitized: String = value.chars().map(|character| {
        if character.is_ascii_control() || ['<', '>', '"', '|', '?', '*'].contains(&character) {
            '_'
        } else {
            character
        }
    }).collect();
    if sanitized.trim_matches([' ', '.']).is_empty() {
        return Err(format!("Недопустимый компонент пути: {value}"));
    }
    Ok(sanitized)
}

fn installation_root(app: &AppHandle, owner: &str, repo: &str, tag: &str, mode: &str) -> Result<PathBuf, String> {
    if mode != "prod" && mode != "dev" {
        return Err("Тип установки должен быть prod или dev.".to_string());
    }
    let owner = sanitize_path_component(owner)?;
    let repo = sanitize_path_component(repo)?;
    let tag = sanitize_path_component(tag)?;
    let state = load_state(app);
    let drive = sanitize_path_component(&state.drive_letter)?;
    if drive.len() != 1 || !drive.chars().next().is_some_and(|character| character.is_ascii_alphabetic()) {
        return Err("Выберите букву диска от A до Z.".to_string());
    }
    Ok(PathBuf::from(format!("{}:\\The_Omniscience", drive.to_ascii_uppercase()))
        .join(owner)
        .join(repo)
        .join(tag)
        .join(mode))
}

fn run_capture(program: &Path, args: &[&str], current_dir: Option<&Path>) -> Result<(bool, i32, String), String> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(directory) = current_dir {
        command.current_dir(directory);
    }
    let output = command.output()
        .map_err(|error| format!("Could not start {}: {error}", program.display()))?;
    let exit_code = output.status.code().unwrap_or(1);
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    Ok((exit_code == 0, exit_code, text))
}

fn github_injection_script() -> String {
    r#"
(() => {
  const injected = 'omniscience-release-install';
  const invoke = (...args) => window.__TAURI_INTERNALS__?.invoke(...args);

  function repoInfo() {
    const parts = location.pathname.split('/').filter(Boolean);
    if (parts.length < 2) return null;
    const owner = parts[0];
    const repo = parts[1];
    const tagIndex = parts.indexOf('tag');
    const tag = tagIndex >= 0 ? parts.slice(tagIndex + 1).join('/') : '';
    return { owner, repo, tag };
  }

  function addButton(link, label, mode) {
    if (!link || link.dataset[injected]) return;
    link.dataset[injected] = '1';
    const info = repoInfo();
    if (!info) return;

    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = label;
    button.style.cssText = 'margin-left:10px;padding:5px 8px;border:1px solid #6e7681;border-radius:6px;background:#238636;color:#fff;font-size:12px;cursor:pointer;';
    button.addEventListener('click', (event) => {
      event.preventDefault();
      event.stopPropagation();
      if (!info.tag) {
        alert('Не удалось определить тег релиза. Откройте конкретную страницу релиза.');
        return;
      }
      if (mode === 'prod') {
        invoke('install_github_artifact', {
          owner: info.owner,
          repo: info.repo,
          tag: info.tag,
          mode,
          assetUrl: new URL(link.href, location.href).href,
          assetName: decodeURIComponent(link.pathname.split('/').pop() || 'Omniscience.exe')
        }).catch((error) => alert('Ошибка установки: ' + error));
      } else {
        invoke('install_github_artifact', {
          owner: info.owner,
          repo: info.repo,
          tag: info.tag,
          mode,
          assetUrl: 'https://github.com/' + info.owner + '/' + info.repo,
          assetName: 'source'
        }).catch((error) => alert('Ошибка установки: ' + error));
      }
    });

    const container = link.parentElement;
    if (container && !container.querySelector('[data-omniscience-button="' + mode + '"]')) {
      button.dataset.omniscienceButton = mode;
      link.insertAdjacentElement('afterend', button);
    }
  }

  function decorate() {
    if (!location.hostname.endsWith('github.com')) return;
    for (const link of document.querySelectorAll('a[href]')) {
      const href = link.getAttribute('href') || '';
      const text = (link.textContent || '').trim().toLowerCase();
      if (/\/releases\/download\/[^/?#]+\/[^/?#]+\.exe(?:$|[?#])/i.test(href) ||
          href.toLowerCase().endsWith('.exe')) {
        addButton(link, 'Установить как Prod', 'prod');
      }
      if (href.includes('/archive/refs/tags/') ||
          href.includes('/archive/') && (text.includes('source code') || text.includes('zip'))) {
        addButton(link, 'Установить как Dev', 'dev');
      }
      if (/source code\s*\(zip\)/i.test(link.textContent || '') ||
          /source code\s*\(tar\.gz\)/i.test(link.textContent || '') ||
          text.includes('source code')) {
        addButton(link, 'Установить как Dev', 'dev');
      }
    }
  }

  const observer = new MutationObserver(decorate);
  observer.observe(document.documentElement, { childList: true, subtree: true });
  decorate();
})();
"#.to_string()
}

#[tauri::command]
fn open_github_browser(app: AppHandle, url: String) -> Result<String, String> {
    let mut url = url.trim().to_string();
    if url.is_empty() {
        url = "https://github.com/EssenceOfDream/The_Omniscience/releases".to_string();
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        url = format!("https://{url}");
    }
    let parsed = Url::parse(&url).map_err(|error| format!("Некорректный URL: {error}"))?;
    if parsed.host_str() != Some("github.com") && parsed.host_str() != Some("www.github.com") {
        return Err("Встроенный браузер ограничен адресами github.com.".to_string());
    }
    if parsed.scheme() != "https" {
        return Err("Для GitHub разрешён только HTTPS.".to_string());
    }

    let mut state = load_state(&app);
    state.browser_url = parsed.to_string();
    save_state(&app, &state)?;

    if let Some(window) = app.get_webview_window("github-browser") {
        window.navigate(parsed).map_err(|error| format!("Не удалось перейти по адресу: {error}"))?;
        window.set_focus().map_err(|error| format!("Не удалось открыть окно: {error}"))?;
    } else {
        WebviewWindowBuilder::new(
            &app,
            "github-browser",
            WebviewUrl::External(parsed),
        )
        .title("Omniscience — GitHub")
        .inner_size(1180.0, 820.0)
        .resizable(true)
        .initialization_script(&github_injection_script())
        .build()
        .map_err(|error| format!("Не удалось открыть встроенный браузер: {error}"))?;
    }
    Ok(state.browser_url)
}

#[tauri::command]
fn navigate_github_browser(app: AppHandle, url: String) -> Result<String, String> {
    open_github_browser(app, url)
}

#[tauri::command]
fn install_github_artifact(app: AppHandle, owner: String, repo: String, tag: String, mode: String, asset_url: String, asset_name: String) -> Result<String, String> {
    let owner = sanitize_path_component(&owner)?;
    let repo = sanitize_path_component(&repo)?;
    let tag = sanitize_path_component(&tag)?;
    if mode != "prod" && mode != "dev" {
        return Err("Тип установки должен быть prod или dev.".to_string());
    }

    let destination = installation_root(&app, &owner, &repo, &tag, &mode)?;
    fs::create_dir_all(&destination)
        .map_err(|error| format!("Не удалось создать каталог установки: {error}"))?;

    if mode == "dev" {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("Не удалось создать каталог: {error}"))?;
        }
        if destination.join(".git").is_dir() {
            let git = PathBuf::from("git");
            let directory = destination.to_string_lossy();
            let status = run_capture(&git, &["-C", &directory, "status", "--porcelain", "--untracked-files=no"], None)?;
            if status.0 && !status.2.trim().is_empty() {
                return Err("Dev-копия содержит изменения. Она не была сброшена.".to_string());
            }
            run_capture(&git, &["-C", &directory, "fetch", "--tags", "--prune", "origin"], None)?;
            let checkout = run_capture(&git, &["-C", &directory, "checkout", "--detach", &tag], None)?;
            if !checkout.0 {
                return Err(format!("Не удалось переключиться на tag {tag}: {}", checkout.2));
            }
        } else {
            if destination.exists() && fs::read_dir(&destination).map(|mut entries| entries.next().is_some()).unwrap_or(false) {
                return Err(format!("Каталог Dev не пуст: {}", destination.display()));
            }
            let git = PathBuf::from("git");
            let clone = run_capture(
                &git,
                &["clone", "--branch", &tag, "--single-branch", &format!("https://github.com/{owner}/{repo}.git"), &destination.to_string_lossy()],
                None,
            )?;
            if !clone.0 {
                return Err(format!("Не удалось клонировать tag {tag}: {}", clone.2));
            }
        }
        let mut state = load_state(&app);
        state.mode = "dev".to_string();
        state.source_directory = destination.to_string_lossy().to_string();
        save_state(&app, &state)?;
        return Ok(destination.to_string_lossy().to_string());
    }

    let parsed = Url::parse(&asset_url).map_err(|error| format!("Некорректный URL файла: {error}"))?;
    let host = parsed.host_str().unwrap_or_default();
    if host != "github.com" && host != "www.github.com" {
        return Err("Prod-файл разрешено скачивать только с github.com.".to_string());
    }
    let asset_name = sanitize_path_component(&asset_name)?;
    if !asset_name.to_ascii_lowercase().ends_with(".exe") {
        return Err("Ожидался production-EXE-файл.".to_string());
    }
    let executable = destination.join(asset_name);
    let curl = PathBuf::from("curl.exe");
    let download = run_capture(
        &curl,
        &["-fL", "--retry", "3", "--max-time", "600", "-o", &executable.to_string_lossy(), parsed.as_str()],
        None,
    )?;
    if !download.0 {
        return Err(format!("Не удалось скачать EXE: {}", download.2));
    }
    let mut state = load_state(&app);
    state.mode = "prod".to_string();
    state.production_executable = executable.to_string_lossy().to_string();
    save_state(&app, &state)?;
    Command::new(&executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("EXE скачан, но установщик не запустился: {error}"))?;
    Ok(executable.to_string_lossy().to_string())
}

#[tauri::command]
fn get_manager_state(app: AppHandle) -> ManagerState {
    load_state(&app)
}

#[tauri::command]
fn set_manager_state(app: AppHandle, mode: String, source_directory: String, production_executable: String, drive_letter: String, browser_url: String) -> Result<ManagerState, String> {
    if mode != "dev" && mode != "prod" {
        return Err("Unsupported mode. Use dev or prod.".to_string());
    }
    let mut drive_letter = drive_letter.trim().to_ascii_uppercase();
    if drive_letter.len() != 1 || !drive_letter.chars().all(|character| character.is_ascii_alphabetic()) {
        return Err("Выберите букву диска от A до Z.".to_string());
    }
    drive_letter = drive_letter[..1].to_string();
    let state = ManagerState {
        mode,
        source_directory: source_directory.trim().to_string(),
        production_executable: production_executable.trim().to_string(),
        drive_letter,
        browser_url: browser_url.trim().to_string(),
    };
    save_state(&app, &state)?;
    Ok(state)
}

#[tauri::command]
fn check_prerequisites(_app: AppHandle) -> Result<Value, String> {
    let root = project_root()?;
    let script = root.join("scripts").join("Test-Prerequisites.ps1");
    if !script.is_file() {
        return Err(format!("Prerequisite script not found: {}", script.display()));
    }
    let powershell = PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
    let powershell = if powershell.is_file() {
        powershell
    } else {
        PathBuf::from("powershell.exe")
    };
    let args = [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        script.to_str().unwrap_or_default(),
        "-Json",
    ];
    let (success, exit_code, text) = run_capture(&powershell, &args, Some(&root))?;
    if !success {
        return Err(format!("Prerequisite check failed with code {exit_code}: {text}"));
    }
    serde_json::from_str(text.trim())
        .map_err(|error| format!("Could not parse prerequisite report: {error}\n{text}"))
}

#[tauri::command]
fn inspect_source(source_directory: String) -> SourceStatus {
    let directory = PathBuf::from(source_directory.trim());
    if !directory.is_dir() {
        return SourceStatus {
            exists: false,
            is_git_repo: false,
            branch: None,
            commit: None,
            tag: None,
            dirty_tracked: false,
            status: "Каталог с исходниками не найден.".to_string(),
        };
    }

    let git = PathBuf::from("git");
    let repo_check = run_capture(&git, &["-C", directory.to_str().unwrap_or_default(), "rev-parse", "--is-inside-work-tree"], None);
    if !matches!(repo_check, Ok((true, _, ref text)) if text.trim() == "true") {
        return SourceStatus {
            exists: true,
            is_git_repo: false,
            branch: None,
            commit: None,
            tag: None,
            dirty_tracked: false,
            status: "Каталог существует, но не является Git-репозиторием.".to_string(),
        };
    }

    let branch = run_capture(&git, &["-C", directory.to_str().unwrap_or_default(), "branch", "--show-current"], None)
        .ok().filter(|result| result.0).map(|result| result.2.trim().to_string()).filter(|value| !value.is_empty());
    let commit = run_capture(&git, &["-C", directory.to_str().unwrap_or_default(), "rev-parse", "HEAD"], None)
        .ok().filter(|result| result.0).map(|result| result.2.trim().to_string()).filter(|value| !value.is_empty());
    let tag = run_capture(&git, &["-C", directory.to_str().unwrap_or_default(), "describe", "--tags", "--exact-match"], None)
        .ok().filter(|result| result.0).map(|result| result.2.trim().to_string()).filter(|value| !value.is_empty());
    let status_output = run_capture(&git, &["-C", directory.to_str().unwrap_or_default(), "status", "--porcelain", "--untracked-files=no"], None);
    let dirty_tracked = matches!(status_output, Ok((true, _, ref text)) if !text.trim().is_empty());
    let status = if dirty_tracked {
        "Есть изменённые отслеживаемые файлы.".to_string()
    } else {
        "Отслеживаемые файлы не изменены.".to_string()
    };

    SourceStatus {
        exists: true,
        is_git_repo: true,
        branch,
        commit,
        tag,
        dirty_tracked,
        status,
    }
}

#[tauri::command]
fn prepare_release(app: AppHandle, tag: String) -> Result<SourceStatus, String> {
    let tag = tag.trim();
    if tag.is_empty() {
        return Err("Укажите release tag.".to_string());
    }
    let state = load_state(&app);
    let directory = PathBuf::from(state.source_directory.trim());
    if directory.as_os_str().is_empty() {
        return Err("Каталог исходников не настроен.".to_string());
    }

    let git = PathBuf::from("git");
    if directory.join(".git").is_dir() {
        let status_output = run_capture(&git, &["-C", directory.to_str().unwrap_or_default(), "status", "--porcelain", "--untracked-files=no"], None)?;
        if status_output.0 && !status_output.2.trim().is_empty() {
            return Err("Рабочее дерево содержит изменения. Сначала сохраните или перенесите их в отдельный commit или ветку; автоматический reset не выполняется.".to_string());
        }
        let fetch = run_capture(&git, &["-C", directory.to_str().unwrap_or_default(), "fetch", "--tags", "--prune", "origin"], None)?;
        if !fetch.0 {
            return Err(format!("Не удалось обновить tags из origin: {}", fetch.2));
        }
        let checkout = run_capture(&git, &["-C", directory.to_str().unwrap_or_default(), "checkout", "--detach", tag], None)?;
        if !checkout.0 {
            return Err(format!("Не удалось переключиться на tag {tag}: {}", checkout.2));
        }
    } else {
        if directory.exists() {
            return Err("Каталог существует, но не является Git-репозиторием. Выберите пустую папку или безопасный checkout.".to_string());
        }
        if let Some(parent) = directory.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("Could not create source parent directory: {error}"))?;
        }
        let clone = run_capture(
            &git,
            &["clone", "--branch", tag, "--single-branch", "https://github.com/EssenceOfDream/The_Omniscience.git", directory.to_str().unwrap_or_default()],
            None,
        )?;
        if !clone.0 {
            return Err(format!("Не удалось клонировать release {tag}: {}", clone.2));
        }
    }

    Ok(inspect_source(directory.to_string_lossy().to_string()))
}

#[tauri::command]
fn check_latest_release() -> ReleaseStatus {
    let curl = PathBuf::from("curl.exe");
    let args = [
        "-fsSL",
        "--max-time",
        "15",
        "https://api.github.com/repos/EssenceOfDream/The_Omniscience/releases/latest",
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        "User-Agent: Omniscience-Updater",
    ];
    match run_capture(&curl, &args, None) {
        Ok((true, _, text)) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => ReleaseStatus {
                available: true,
                tag: value["tag_name"].as_str().map(str::to_string),
                name: value["name"].as_str().map(str::to_string).or_else(|| value["tag_name"].as_str().map(str::to_string)),
                published_at: value["published_at"].as_str().map(str::to_string),
                url: value["html_url"].as_str().map(str::to_string),
                error: None,
            },
            Err(error) => ReleaseStatus {
                available: false,
                tag: None,
                name: None,
                published_at: None,
                url: None,
                error: Some(format!("Не удалось разобрать ответ GitHub: {error}")),
            },
        },
        Ok((false, exit_code, text)) => {
            let detail = text.trim();
            ReleaseStatus {
                available: false,
                tag: None,
                name: None,
                published_at: None,
                url: None,
                error: Some(format!("GitHub не ответил (код {exit_code}): {detail}")),
            }
        },
        Err(error) => ReleaseStatus {
            available: false,
            tag: None,
            name: None,
            published_at: None,
            url: None,
            error: Some(error),
        },
    }
}

#[tauri::command]
fn launch_omniscience(app: AppHandle) -> Result<String, String> {
    let state = load_state(&app);
    if state.mode == "dev" {
        let directory = PathBuf::from(state.source_directory);
        if !directory.join("package.json").is_file() {
            return Err("Dev-режим требует каталог исходников с package.json.".to_string());
        }
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        Command::new(npm)
            .args(["run", "tauri", "dev"])
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Не удалось запустить Dev: {error}"))?;
        Ok("Dev-запуск инициирован.".to_string())
    } else {
        let executable = PathBuf::from(state.production_executable);
        if !executable.is_file() {
            return Err("Production EXE не настроен или файл отсутствует.".to_string());
        }
        Command::new(executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Не удалось запустить Prod: {error}"))?;
        Ok("Production-запуск инициирован.".to_string())
    }
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
        .invoke_handler(tauri::generate_handler![
            start_install,
            get_manager_state,
            set_manager_state,
            check_prerequisites,
            inspect_source,
            prepare_release,
            check_latest_release,
            launch_omniscience,
            open_github_browser,
            navigate_github_browser,
            install_github_artifact
        ])
        .run(tauri::generate_context!())
        .expect("error while running Omniscience Updater");
}
