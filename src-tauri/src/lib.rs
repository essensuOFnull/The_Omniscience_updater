use std::{
    collections::HashSet,
    env,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, Url, WebviewBuilder,
    WebviewUrl, WindowBuilder,
};

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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct InstalledVersion {
    mode: String,
    owner: String,
    repo: String,
    tag: String,
    path: String,
}

fn project_root() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| "Could not resolve updater project root.".to_string())
}

fn emit_output(app: &AppHandle, text: String) {
    let text = match append_output_log(app, &text) {
        Ok(()) => text,
        Err(error) => format!("{text}\n[Не удалось сохранить полный журнал: {error}]\n"),
    };
    let _ = app.emit("installer-output", serde_json::json!({ "text": text }));
}

fn output_log_path(app: &AppHandle) -> Result<PathBuf, String> {
    static SESSION_TIMESTAMP: OnceLock<u128> = OnceLock::new();
    let timestamp = SESSION_TIMESTAMP.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    });
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Не удалось определить каталог данных приложения: {error}"))?
        .join("logs");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Не удалось создать каталог журнала: {error}"))?;
    Ok(directory.join(format!(
        "updater-output-{}-{timestamp}.log",
        std::process::id()
    )))
}

fn append_output_log(app: &AppHandle, text: &str) -> Result<(), String> {
    let path = output_log_path(app)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("Не удалось открыть {}: {error}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("Не удалось записать {}: {error}", path.display()))
}

fn emit_installed_version_changed(app: &AppHandle) -> Result<(), String> {
    app.emit("installed-version-changed", serde_json::json!({}))
        .map_err(|error| format!("Не удалось обновить список установок: {error}"))
}

fn state_file(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_data_dir()
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
    fs::write(path, text).map_err(|error| format!("Could not save manager state: {error}"))
}

fn sanitize_path_component(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Path component is empty.".to_string());
    }
    if value == "." || value == ".." || value.contains(['/', '\\', ':']) {
        return Err(format!("Недопустимый компонент пути: {value}"));
    }
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_control() || ['<', '>', '"', '|', '?', '*'].contains(&character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    if sanitized.trim_matches([' ', '.']).is_empty() {
        return Err(format!("Недопустимый компонент пути: {value}"));
    }
    Ok(sanitized)
}

fn installation_base(drive_letter: &str) -> Result<PathBuf, String> {
    let drive = drive_letter.trim();
    if drive.len() != 1
        || !drive
            .chars()
            .all(|character| character.is_ascii_alphabetic())
    {
        return Err("Выберите букву диска от A до Z.".to_string());
    }
    Ok(PathBuf::from(format!(
        "{}:\\The_Omniscience",
        drive.to_ascii_uppercase()
    )))
}

fn installation_root(
    app: &AppHandle,
    owner: &str,
    repo: &str,
    tag: &str,
    mode: &str,
) -> Result<PathBuf, String> {
    if mode != "prod" && mode != "dev" {
        return Err("Тип установки должен быть prod или dev.".to_string());
    }
    let owner = sanitize_path_component(owner)?;
    let repo = sanitize_path_component(repo)?;
    let tag = sanitize_path_component(tag)?;
    let state = load_state(app);
    Ok(installation_base(&state.drive_letter)?
        .join(owner)
        .join(repo)
        .join(tag)
        .join(mode))
}

fn immediate_directories(path: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!("Не удалось прочитать {}: {error}", path.display()));
        }
    };
    let mut directories = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("Не удалось прочитать каталог: {error}"))?;
        if entry
            .file_type()
            .map_err(|error| format!("Не удалось проверить {}: {error}", entry.path().display()))?
            .is_dir()
        {
            directories.push(entry.path());
        }
    }
    Ok(directories)
}

fn path_is_directory(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Не удалось проверить {}: {error}", path.display())),
    }
}

fn path_is_file(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Не удалось проверить {}: {error}", path.display())),
    }
}

fn scan_installed_versions(root: &Path, mode: &str) -> Result<Vec<InstalledVersion>, String> {
    if mode != "dev" && mode != "prod" {
        return Err("Тип установки должен быть prod или dev.".to_string());
    }

    let mut installed = Vec::new();
    for owner_path in immediate_directories(root)? {
        let owner = owner_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for repo_path in immediate_directories(&owner_path)? {
            let repo = repo_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            for tag_path in immediate_directories(&repo_path)? {
                let tag = tag_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let install_path = tag_path.join(mode);
                if !path_is_directory(&install_path)? {
                    continue;
                }

                if mode == "dev" {
                    if path_is_directory(&install_path.join(".git"))?
                        && path_is_file(&install_path.join("package.json"))?
                    {
                        installed.push(InstalledVersion {
                            mode: mode.to_string(),
                            owner: owner.clone(),
                            repo: repo.clone(),
                            tag,
                            path: install_path.to_string_lossy().into_owned(),
                        });
                    }
                    continue;
                }

                let entries = fs::read_dir(&install_path).map_err(|error| {
                    format!("Не удалось прочитать {}: {error}", install_path.display())
                })?;
                for entry in entries {
                    let entry =
                        entry.map_err(|error| format!("Не удалось прочитать каталог: {error}"))?;
                    if !entry
                        .file_type()
                        .map_err(|error| {
                            format!("Не удалось проверить {}: {error}", entry.path().display())
                        })?
                        .is_file()
                    {
                        continue;
                    }
                    if entry.path().extension().is_some_and(|extension| {
                        extension.to_string_lossy().eq_ignore_ascii_case("exe")
                    }) {
                        installed.push(InstalledVersion {
                            mode: mode.to_string(),
                            owner: owner.clone(),
                            repo: repo.clone(),
                            tag: tag.clone(),
                            path: entry.path().to_string_lossy().into_owned(),
                        });
                    }
                }
            }
        }
    }
    installed.sort_by(|left, right| {
        (&left.owner, &left.repo, &left.tag, &left.path).cmp(&(
            &right.owner,
            &right.repo,
            &right.tag,
            &right.path,
        ))
    });
    Ok(installed)
}

fn run_capture(
    program: &Path,
    args: &[&str],
    current_dir: Option<&Path>,
) -> Result<(bool, i32, String), String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = current_dir {
        command.current_dir(directory);
    }
    let output = command
        .output()
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
  const invoke = (...args) => window.__TAURI_INTERNALS__?.invoke(...args);

  function repoInfo() {
    const parts = location.pathname.split('/').filter(Boolean);
    if (parts.length < 2) return null;
    return { owner: parts[0], repo: parts[1] };
  }

  function findTag(link) {
    const downloadMatch = link.href.match(/\/releases\/download\/([^/?#]+)\//i);
    if (downloadMatch) return decodeURIComponent(downloadMatch[1]);
    const archiveMatch = link.href.match(/\/archive\/(?:refs\/tags\/|[^/]*\/)([^/?#]+)\.(?:zip|tar\.gz)/i);
    if (archiveMatch) return decodeURIComponent(archiveMatch[1].replace(/\.zip$|\.tar\.gz$/i, ''));

    let element = link;
    for (let depth = 0; element && depth < 8; depth++, element = element.parentElement) {
      const tagLink = element.querySelector?.('a[href*="/releases/tag/"]');
      const match = tagLink?.getAttribute('href')?.match(/\/releases\/tag\/([^/?#]+)$/i);
      if (match) return decodeURIComponent(match[1]);
    }
    return null;
  }

  function addButton(link, mode) {
    if (link.dataset.omniscienceProcessed) return;
    const info = repoInfo();
    const tag = findTag(link);
    if (!info || !tag) return;
    link.dataset.omniscienceProcessed = '1';

    const button = document.createElement('button');
    button.type = 'button';
    const installLabel = mode === 'prod' ? 'Установить как Prod' : 'Установить как Dev';
    button.textContent = 'Проверка…';
    button.disabled = true;
    button.style.cssText = 'margin-left:10px;padding:5px 8px;border:1px solid #6e7681;border-radius:6px;background:#238636;color:#fff;font-size:12px;cursor:pointer;';
    const status = document.createElement('span');
    status.style.cssText = 'margin-left:6px;color:#8b949e;font-size:12px;';
    button.addEventListener('click', async (event) => {
      event.preventDefault();
      event.stopPropagation();
      button.disabled = true;
      button.textContent = 'Установка…';
      status.textContent = '';
      try {
        await invoke('install_github_artifact', {
          owner: info.owner,
          repo: info.repo,
          tag,
          mode,
          assetUrl: mode === 'prod'
            ? new URL(link.href, location.href).href
            : 'https://github.com/' + info.owner + '/' + info.repo,
          assetName: mode === 'prod'
            ? decodeURIComponent(link.pathname.split('/').pop() || 'Omniscience.exe')
            : 'source'
        });
        button.dataset.installed = 'true';
        button.textContent = 'Переустановить';
        status.textContent = 'Установлена';
      } catch (error) {
        button.textContent = button.dataset.installed === 'true' ? 'Переустановить' : installLabel;
        status.textContent = 'Ошибка установки';
        status.title = String(error);
      } finally {
        button.disabled = false;
      }
    });
    link.insertAdjacentElement('afterend', button);
    button.insertAdjacentElement('afterend', status);

    (async () => {
      try {
        const installed = await invoke('is_github_artifact_installed', {
          owner: info.owner,
          repo: info.repo,
          tag,
          mode,
          assetName: mode === 'prod'
            ? decodeURIComponent(link.pathname.split('/').pop() || 'Omniscience.exe')
            : 'source'
        });
        button.dataset.installed = String(installed);
        button.textContent = installed ? 'Переустановить' : installLabel;
        status.textContent = installed ? 'Установлена' : '';
      } catch (error) {
        button.textContent = installLabel;
        status.textContent = 'Не удалось проверить установку';
        status.title = String(error);
      } finally {
        button.disabled = false;
      }
    })();
  }

  function decorate() {
    if (!location.hostname.endsWith('github.com')) return;
    for (const link of document.querySelectorAll('a[href]')) {
      const href = link.getAttribute('href') || '';
      const text = (link.textContent || '').trim();
      if (/\/releases\/download\/[^/?#]+\/[^/?#]+\.exe(?:$|[?#])/i.test(href)
          || href.toLowerCase().endsWith('.exe')) {
        addButton(link, 'prod');
      }
      if (/source code\s*\((?:zip|tar\.gz)\)/i.test(text)
          || /\/archive\/(?:refs\/tags\/|[^/]+\/)/i.test(href)
          && /zip|tar\.gz/i.test(href)) {
        addButton(link, 'dev');
      }
    }
  }

  decorate();
  const observer = new MutationObserver(decorate);
  if (document.documentElement) {
    observer.observe(document.documentElement, { childList: true, subtree: true });
  }
})();
"#.to_string()
}

#[tauri::command]
async fn open_github_browser(app: AppHandle, url: String) -> Result<String, String> {
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

    let window = match (
        app.get_window("github-browser"),
        app.get_webview("github-browser"),
    ) {
        (Some(window), Some(webview)) => {
            webview
                .navigate(parsed.clone())
                .map_err(|error| format!("Не удалось перейти по адресу: {error}"))?;
            window
        }
        (None, None) => {
            let window = WindowBuilder::new(&app, "github-browser")
                .title("Omniscience — загрузка GitHub")
                .inner_size(1180.0, 820.0)
                .resizable(true)
                .visible(false)
                .build()
                .map_err(|error| format!("Не удалось создать окно GitHub: {error}"))?;
            attach_github_webview(&app, &window, parsed)?;
            window
        }
        _ => {
            return Err(
                "Не удалось восстановить окно GitHub: состояние окна неполное.".to_string(),
            );
        }
    };
    window
        .show()
        .map_err(|error| format!("Не удалось показать окно GitHub: {error}"))?;
    window
        .set_focus()
        .map_err(|error| format!("Не удалось сфокусировать окно GitHub: {error}"))?;
    Ok(state.browser_url)
}

fn attach_github_webview(app: &AppHandle, window: &tauri::Window, url: Url) -> Result<(), String> {
    let app_handle = app.clone();
    let webview = WebviewBuilder::new("github-browser", WebviewUrl::External(url))
        .devtools(true)
        .on_page_load(move |webview, payload| {
            let (event, title) = match payload.event() {
                tauri::webview::PageLoadEvent::Started => {
                    ("started", "Omniscience — загрузка GitHub")
                }
                tauri::webview::PageLoadEvent::Finished => {
                    ("finished", "Omniscience — GitHub (загружено)")
                }
            };
            if let Some(window) = app_handle.get_window("github-browser") {
                let _ = window.set_title(title);
            }
            let _ = app_handle.emit(
                "github-browser-load",
                serde_json::json!({
                    "event": event,
                    "url": payload.url().as_str(),
                }),
            );
            if event == "finished" {
                let _ = webview.eval(github_injection_script());
            }
        });

    window
        .add_child(
            webview,
            LogicalPosition::new(0.0, 0.0),
            LogicalSize::new(1180.0, 820.0),
        )
        .map_err(|error| format!("Не удалось добавить GitHub WebView: {error}"))?;
    Ok(())
}

#[tauri::command]
async fn navigate_github_browser(app: AppHandle, url: String) -> Result<String, String> {
    open_github_browser(app, url).await
}

#[tauri::command]
fn open_github_devtools(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview("github-browser")
        .ok_or_else(|| "Окно GitHub ещё не открыто.".to_string())?;
    window.open_devtools();
    Ok(())
}

#[tauri::command]
fn install_github_artifact(
    app: AppHandle,
    owner: String,
    repo: String,
    tag: String,
    mode: String,
    asset_url: String,
    asset_name: String,
) -> Result<String, String> {
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
            fs::create_dir_all(parent)
                .map_err(|error| format!("Не удалось создать каталог: {error}"))?;
        }
        if destination.join(".git").is_dir() {
            let git = PathBuf::from("git");
            let directory = destination.to_string_lossy();
            let status = run_capture(
                &git,
                &[
                    "-C",
                    &directory,
                    "status",
                    "--porcelain",
                    "--untracked-files=no",
                ],
                None,
            )?;
            if status.0 && !status.2.trim().is_empty() {
                return Err("Dev-копия содержит изменения. Она не была сброшена.".to_string());
            }
            run_capture(
                &git,
                &["-C", &directory, "fetch", "--tags", "--prune", "origin"],
                None,
            )?;
            let checkout = run_capture(
                &git,
                &["-C", &directory, "checkout", "--detach", &tag],
                None,
            )?;
            if !checkout.0 {
                return Err(format!(
                    "Не удалось переключиться на tag {tag}: {}",
                    checkout.2
                ));
            }
        } else {
            if destination.exists()
                && fs::read_dir(&destination)
                    .map(|mut entries| entries.next().is_some())
                    .unwrap_or(false)
            {
                return Err(format!("Каталог Dev не пуст: {}", destination.display()));
            }
            let git = PathBuf::from("git");
            let clone = run_capture(
                &git,
                &[
                    "clone",
                    "--branch",
                    &tag,
                    "--single-branch",
                    &format!("https://github.com/{owner}/{repo}.git"),
                    &destination.to_string_lossy(),
                ],
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
        emit_installed_version_changed(&app)?;
        return Ok(destination.to_string_lossy().to_string());
    }

    let parsed =
        Url::parse(&asset_url).map_err(|error| format!("Некорректный URL файла: {error}"))?;
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
        &[
            "-fL",
            "--retry",
            "3",
            "--max-time",
            "600",
            "-o",
            &executable.to_string_lossy(),
            parsed.as_str(),
        ],
        None,
    )?;
    if !download.0 {
        return Err(format!("Не удалось скачать EXE: {}", download.2));
    }
    let mut state = load_state(&app);
    state.mode = "prod".to_string();
    state.production_executable = executable.to_string_lossy().to_string();
    save_state(&app, &state)?;
    emit_installed_version_changed(&app)?;
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
fn get_output_log_path(app: AppHandle) -> Result<String, String> {
    output_log_path(&app).map(|path| path.to_string_lossy().into_owned())
}

#[tauri::command]
fn list_installed_versions(app: AppHandle, mode: String) -> Result<Vec<InstalledVersion>, String> {
    let state = load_state(&app);
    let root = installation_base(&state.drive_letter)?;
    scan_installed_versions(&root, &mode)
}

#[tauri::command]
fn is_github_artifact_installed(
    app: AppHandle,
    owner: String,
    repo: String,
    tag: String,
    mode: String,
    asset_name: String,
) -> Result<bool, String> {
    let directory = installation_root(&app, &owner, &repo, &tag, &mode)?;
    if mode == "dev" {
        return Ok(path_is_directory(&directory.join(".git"))?
            && path_is_file(&directory.join("package.json"))?);
    }

    let asset_name = sanitize_path_component(&asset_name)?;
    if !asset_name.to_ascii_lowercase().ends_with(".exe") {
        return Err("Ожидался production-EXE-файл.".to_string());
    }
    path_is_file(&directory.join(asset_name))
}

#[tauri::command]
fn set_manager_state(
    app: AppHandle,
    mode: String,
    source_directory: String,
    production_executable: String,
    drive_letter: String,
    browser_url: String,
) -> Result<ManagerState, String> {
    if mode != "dev" && mode != "prod" {
        return Err("Unsupported mode. Use dev or prod.".to_string());
    }
    let mut drive_letter = drive_letter.trim().to_ascii_uppercase();
    if drive_letter.len() != 1
        || !drive_letter
            .chars()
            .all(|character| character.is_ascii_alphabetic())
    {
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
        return Err(format!(
            "Prerequisite script not found: {}",
            script.display()
        ));
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
        return Err(format!(
            "Prerequisite check failed with code {exit_code}: {text}"
        ));
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
    let repo_check = run_capture(
        &git,
        &[
            "-C",
            directory.to_str().unwrap_or_default(),
            "rev-parse",
            "--is-inside-work-tree",
        ],
        None,
    );
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

    let branch = run_capture(
        &git,
        &[
            "-C",
            directory.to_str().unwrap_or_default(),
            "branch",
            "--show-current",
        ],
        None,
    )
    .ok()
    .filter(|result| result.0)
    .map(|result| result.2.trim().to_string())
    .filter(|value| !value.is_empty());
    let commit = run_capture(
        &git,
        &[
            "-C",
            directory.to_str().unwrap_or_default(),
            "rev-parse",
            "HEAD",
        ],
        None,
    )
    .ok()
    .filter(|result| result.0)
    .map(|result| result.2.trim().to_string())
    .filter(|value| !value.is_empty());
    let tag = run_capture(
        &git,
        &[
            "-C",
            directory.to_str().unwrap_or_default(),
            "describe",
            "--tags",
            "--exact-match",
        ],
        None,
    )
    .ok()
    .filter(|result| result.0)
    .map(|result| result.2.trim().to_string())
    .filter(|value| !value.is_empty());
    let status_output = run_capture(
        &git,
        &[
            "-C",
            directory.to_str().unwrap_or_default(),
            "status",
            "--porcelain",
            "--untracked-files=no",
        ],
        None,
    );
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
        let status_output = run_capture(
            &git,
            &[
                "-C",
                directory.to_str().unwrap_or_default(),
                "status",
                "--porcelain",
                "--untracked-files=no",
            ],
            None,
        )?;
        if status_output.0 && !status_output.2.trim().is_empty() {
            return Err("Рабочее дерево содержит изменения. Сначала сохраните или перенесите их в отдельный commit или ветку; автоматический reset не выполняется.".to_string());
        }
        let fetch = run_capture(
            &git,
            &[
                "-C",
                directory.to_str().unwrap_or_default(),
                "fetch",
                "--tags",
                "--prune",
                "origin",
            ],
            None,
        )?;
        if !fetch.0 {
            return Err(format!("Не удалось обновить tags из origin: {}", fetch.2));
        }
        let checkout = run_capture(
            &git,
            &[
                "-C",
                directory.to_str().unwrap_or_default(),
                "checkout",
                "--detach",
                tag,
            ],
            None,
        )?;
        if !checkout.0 {
            return Err(format!(
                "Не удалось переключиться на tag {tag}: {}",
                checkout.2
            ));
        }
    } else {
        if directory.exists() {
            return Err("Каталог существует, но не является Git-репозиторием. Выберите пустую папку или безопасный checkout.".to_string());
        }
        if let Some(parent) = directory.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create source parent directory: {error}"))?;
        }
        let clone = run_capture(
            &git,
            &[
                "clone",
                "--branch",
                tag,
                "--single-branch",
                "https://github.com/EssenceOfDream/The_Omniscience.git",
                directory.to_str().unwrap_or_default(),
            ],
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
                name: value["name"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| value["tag_name"].as_str().map(str::to_string)),
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
        }
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
fn find_executable_on_path(name: &str) -> Option<PathBuf> {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

fn node_is_compatible(version: &str) -> bool {
    let version = version.trim().trim_start_matches('v');
    let mut parts = version
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok());
    let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
        return false;
    };
    major > 22 || (major == 22 && minor >= 12) || (major == 20 && minor >= 19)
}

fn node_installation() -> Option<(PathBuf, PathBuf)> {
    let mut candidates = Vec::new();
    if let Some(node) = find_executable_on_path(if cfg!(windows) { "node.exe" } else { "node" }) {
        candidates.push(node);
    }
    if cfg!(windows) {
        if let Some(program_files) = env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(program_files).join("nodejs").join("node.exe"));
        }
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local_app_data)
                    .join("Programs")
                    .join("nodejs")
                    .join("node.exe"),
            );
        }
    }

    candidates.into_iter().find_map(|node| {
        if !node.is_file() {
            return None;
        }
        let npm_cli = node
            .parent()?
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join("npm-cli.js");
        npm_cli.is_file().then_some((node, npm_cli))
    })
}

fn node_installation_is_compatible(installation: Option<&(PathBuf, PathBuf)>) -> bool {
    installation
        .and_then(|(node, _)| {
            Command::new(node)
                .arg("--version")
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| node_is_compatible(&String::from_utf8_lossy(&output.stdout)))
        })
        .unwrap_or(false)
}

fn prepend_to_path(
    path: &Path,
    current: Option<&std::ffi::OsStr>,
) -> Result<std::ffi::OsString, String> {
    let mut paths = vec![path.to_path_buf()];
    if let Some(current) = current {
        paths.extend(env::split_paths(current));
    } else if let Some(current) = env::var_os("PATH") {
        paths.extend(env::split_paths(&current));
    }
    env::join_paths(paths).map_err(|error| format!("Не удалось обновить PATH процесса: {error}"))
}

fn stream_child_output<R: Read + Send + 'static>(
    app: AppHandle,
    reader: R,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let (sender, receiver) = mpsc::sync_channel(256);
        thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(format!("{line}\n")).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(format!("\nОшибка чтения вывода процесса: {error}\n"));
                        break;
                    }
                }
            }
        });

        let mut batch = String::new();
        let flush_interval = Duration::from_millis(50);
        let mut flush_at = Instant::now() + flush_interval;
        loop {
            if Instant::now() >= flush_at {
                if !batch.is_empty() {
                    emit_output(&app, std::mem::take(&mut batch));
                }
                flush_at = Instant::now() + flush_interval;
                continue;
            }

            match receiver.recv_timeout(flush_at.saturating_duration_since(Instant::now())) {
                Ok(text) => {
                    batch.push_str(&text);
                    if batch.len() >= 65536 {
                        thread::sleep(flush_at.saturating_duration_since(Instant::now()));
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if !batch.is_empty() {
                        emit_output(&app, batch);
                    }
                    break;
                }
            }
        }
    })
}

fn run_logged_command(
    app: &AppHandle,
    program: &Path,
    args: &[&str],
    current_dir: Option<&Path>,
    path: Option<&std::ffi::OsStr>,
    description: &str,
) -> Result<(), String> {
    emit_output(app, format!("\n{description}…\n"));
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = current_dir {
        command.current_dir(directory);
    }
    if let Some(path) = path {
        command.env("PATH", path);
    }
    run_logged_process(app, command, description)
}

fn run_logged_process(
    app: &AppHandle,
    mut command: Command,
    description: &str,
) -> Result<(), String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("Не удалось запустить {description}: {error}"))?;
    let stdout_reader = child
        .stdout
        .take()
        .map(|stdout| stream_child_output(app.clone(), stdout));
    let stderr_reader = child
        .stderr
        .take()
        .map(|stderr| stream_child_output(app.clone(), stderr));
    let status = child
        .wait()
        .map_err(|error| format!("Не удалось дождаться завершения {description}: {error}"))?;
    if let Some(reader) = stdout_reader {
        let _ = reader.join();
    }
    if let Some(reader) = stderr_reader {
        let _ = reader.join();
    }
    if !status.success() {
        return Err(format!(
            "{description} завершился с кодом {}.",
            status.code().unwrap_or(1)
        ));
    }
    Ok(())
}

fn winget_executable() -> Option<PathBuf> {
    find_executable_on_path(if cfg!(windows) {
        "winget.exe"
    } else {
        "winget"
    })
    .or_else(|| {
        env::var_os("LOCALAPPDATA")
            .map(|local_app_data| {
                PathBuf::from(local_app_data)
                    .join("Microsoft")
                    .join("WindowsApps")
                    .join("winget.exe")
            })
            .filter(|path| path.is_file())
    })
}

fn ensure_node(app: &AppHandle) -> Result<(PathBuf, PathBuf, std::ffi::OsString), String> {
    let mut installation = node_installation();
    if !node_installation_is_compatible(installation.as_ref()) {
        let winget = winget_executable()
            .ok_or_else(|| "Для установки Node.js требуется winget (App Installer).".to_string())?;
        if installation.is_some() {
            run_logged_command(
                app,
                &winget,
                &[
                    "upgrade",
                    "--id",
                    "OpenJS.NodeJS.LTS",
                    "--exact",
                    "--silent",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                ],
                None,
                None,
                "Обновление Node.js LTS через winget",
            )?;
            installation = node_installation();
        } else {
            run_logged_command(
                app,
                &winget,
                &[
                    "install",
                    "--id",
                    "OpenJS.NodeJS.LTS",
                    "--exact",
                    "--silent",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                ],
                None,
                None,
                "Установка совместимого Node.js LTS через winget",
            )?;
            installation = node_installation();
        }
    }

    if !node_installation_is_compatible(installation.as_ref()) {
        return Err(
            "Не удалось установить совместимый Node.js LTS (нужен 20.19+ или 22.12+).".to_string(),
        );
    }

    let (node, npm_cli) = installation
        .ok_or_else(|| "Node.js установлен, но node.exe или npm-cli.js не найден.".to_string())?;
    let version = Command::new(&node)
        .arg("--version")
        .output()
        .map_err(|error| format!("Не удалось проверить Node.js: {error}"))?;
    if !version.status.success() {
        return Err("Не удалось получить версию Node.js.".to_string());
    }
    let version = String::from_utf8_lossy(&version.stdout);
    if !node_is_compatible(&version) {
        return Err(format!(
            "Node.js {version} несовместим с Vite 8 (требуется 20.19+ или 22.12+)."
        ));
    }
    let node_directory = node
        .parent()
        .ok_or_else(|| "Не удалось определить каталог node.exe.".to_string())?;
    let path = prepend_to_path(node_directory, None)?;
    Ok((node, npm_cli, path))
}

fn visual_cpp_tools_installed() -> bool {
    let program_files_x86 = env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
    let vswhere = program_files_x86
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    if !vswhere.is_file() {
        return false;
    }
    Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

fn ensure_dev_toolchain(
    app: &AppHandle,
    path: &std::ffi::OsStr,
) -> Result<std::ffi::OsString, String> {
    let cargo_bin = env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".cargo")
        .join("bin");
    let rustup = find_executable_on_path(if cfg!(windows) {
        "rustup.exe"
    } else {
        "rustup"
    })
    .or_else(|| {
        let candidate = cargo_bin.join(if cfg!(windows) {
            "rustup.exe"
        } else {
            "rustup"
        });
        candidate.is_file().then_some(candidate)
    });

    let rustup = if let Some(rustup) = rustup {
        rustup
    } else {
        let winget = winget_executable()
            .ok_or_else(|| "Для установки Rust требуется winget (App Installer).".to_string())?;
        run_logged_command(
            app,
            &winget,
            &[
                "install",
                "--id",
                "Rustlang.Rustup",
                "--exact",
                "--silent",
                "--accept-package-agreements",
                "--accept-source-agreements",
            ],
            None,
            Some(path),
            "Установка rustup через winget",
        )?;
        let candidate = cargo_bin.join(if cfg!(windows) {
            "rustup.exe"
        } else {
            "rustup"
        });
        if !candidate.is_file() {
            return Err("Rustup установлен, но rustup.exe не найден.".to_string());
        }
        candidate
    };

    run_logged_command(
        app,
        &rustup,
        &["toolchain", "install", "stable-x86_64-pc-windows-msvc"],
        None,
        Some(path),
        "Подготовка Rust stable MSVC",
    )?;
    run_logged_command(
        app,
        &rustup,
        &["default", "stable-x86_64-pc-windows-msvc"],
        None,
        Some(path),
        "Выбор toolchain Rust MSVC",
    )?;

    if !visual_cpp_tools_installed() {
        let winget = winget_executable()
            .ok_or_else(|| "Для установки C++ Build Tools требуется winget.".to_string())?;
        run_logged_command(
            app,
            &winget,
            &[
                "install",
                "--id",
                "Microsoft.VisualStudio.2022.BuildTools",
                "--exact",
                "--silent",
                "--accept-package-agreements",
                "--accept-source-agreements",
                "--override",
                "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended",
            ],
            None,
            Some(path),
            "Установка Visual C++ Build Tools и Windows SDK через winget",
        )?;
        if !visual_cpp_tools_installed() {
            return Err("Установка завершилась, но MSVC Build Tools не обнаружены.".to_string());
        }
    }
    prepend_to_path(&cargo_bin, Some(path))
}

fn launch_dev(app: &AppHandle, directory: &Path) -> Result<(), String> {
    if !directory.join("package.json").is_file() {
        return Err("Dev-режим требует каталог исходников с package.json.".to_string());
    }

    let (node, npm_cli, path) = ensure_node(app)?;
    let path = ensure_dev_toolchain(app, &path)?;
    let mut npm_install = Command::new(&node);
    npm_install
        .arg(&npm_cli)
        .args(["install", "--no-audit", "--no-fund"])
        .current_dir(directory)
        .env("PATH", &path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_logged_process(app, npm_install, "Установка npm-зависимостей (npm install)")?;

    let port = TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map_err(|error| format!("Не удалось выделить порт для Omniscience Dev: {error}"))?
        .port();
    let tauri_config = tauri_dev_config(port).to_string();

    emit_output(
        app,
        format!("\nЗапуск Omniscience Dev на порту {port} (updater использует 22869)…\n"),
    );
    let mut child = Command::new(&node)
        .arg(&npm_cli)
        .args(["run", "tauri", "dev"])
        .current_dir(directory)
        .env("PATH", &path)
        .env("TAURI_CONFIG", tauri_config)
        .env("OMNISCIENCE_DEV_PORT", port.to_string())
        .env("CARGO_TERM_COLOR", "always")
        .env("CARGO_TERM_PROGRESS_WHEN", "never")
        .env("FORCE_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Не удалось запустить Tauri Dev: {error}"))?;
    if let Some(stdout) = child.stdout.take() {
        stream_child_output(app.clone(), stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        stream_child_output(app.clone(), stderr);
    }
    let app_handle = app.clone();
    thread::spawn(move || match child.wait() {
        Ok(status) => {
            let _ = app_handle.emit(
                "dev-process-finished",
                serde_json::json!({
                    "success": status.success(),
                    "exitCode": status.code().unwrap_or(1),
                }),
            );
        }
        Err(error) => {
            let _ = app_handle.emit(
                "dev-process-finished",
                serde_json::json!({
                    "success": false,
                    "exitCode": 1,
                    "error": error.to_string(),
                }),
            );
        }
    });
    emit_output(app, "Команда npm run tauri dev запущена.\n".to_string());
    Ok(())
}

fn tauri_dev_config(port: u16) -> Value {
    serde_json::json!({
        "build": {
            "devUrl": format!("http://127.0.0.1:{port}"),
            "beforeDevCommand": format!(
                "npm run build:project && npm run preview -- --host 127.0.0.1 --port {port} --strictPort"
            )
        }
    })
}

fn append_executables_from_directory(
    directory: &Path,
    executable_paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Не удалось прочитать {}: {error}",
                directory.display()
            ));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Не удалось прочитать каталог {}: {error}",
                directory.display()
            )
        })?;
        if entry
            .file_type()
            .map_err(|error| format!("Не удалось проверить {}: {error}", entry.path().display()))?
            .is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("exe"))
        {
            executable_paths.push(entry.path());
        }
    }
    Ok(())
}

fn omniscience_executable_paths(state: &ManagerState) -> Result<Vec<PathBuf>, String> {
    let mut installation_roots = vec![installation_base(&state.drive_letter)?];
    for drive_letter in 'A'..='Z' {
        let root = installation_base(&drive_letter.to_string())?;
        if root.is_dir() && !installation_roots.contains(&root) {
            installation_roots.push(root);
        }
    }

    let mut executable_paths = Vec::new();
    let mut development_directories = Vec::new();
    for installation_root in installation_roots {
        let production_versions = scan_installed_versions(&installation_root, "prod")?;
        let installed_development = scan_installed_versions(&installation_root, "dev")?;
        executable_paths.extend(
            production_versions
                .into_iter()
                .map(|version| PathBuf::from(version.path)),
        );
        development_directories.extend(
            installed_development
                .into_iter()
                .map(|version| PathBuf::from(version.path)),
        );
    }
    append_development_executables(state, development_directories, &mut executable_paths)?;
    Ok(deduplicate_executable_paths(executable_paths))
}

#[cfg(test)]
fn omniscience_executable_paths_in_root(
    state: &ManagerState,
    installation_root: &Path,
) -> Result<Vec<PathBuf>, String> {
    let production_versions = scan_installed_versions(installation_root, "prod")?;
    let development_versions = scan_installed_versions(installation_root, "dev")?;
    let mut executable_paths: Vec<PathBuf> = production_versions
        .into_iter()
        .map(|version| PathBuf::from(version.path))
        .collect();

    let development_directories: Vec<PathBuf> = development_versions
        .into_iter()
        .map(|version| PathBuf::from(version.path))
        .collect();
    append_development_executables(state, development_directories, &mut executable_paths)?;
    Ok(deduplicate_executable_paths(executable_paths))
}

fn append_development_executables(
    state: &ManagerState,
    mut development_directories: Vec<PathBuf>,
    executable_paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if !state.source_directory.trim().is_empty() {
        development_directories.push(PathBuf::from(state.source_directory.trim()));
    }
    for directory in development_directories {
        append_executables_from_directory(
            &directory.join("src-tauri").join("target").join("debug"),
            executable_paths,
        )?;
    }
    if !state.production_executable.trim().is_empty() {
        executable_paths.push(PathBuf::from(state.production_executable.trim()));
    }
    Ok(())
}

fn deduplicate_executable_paths(mut executable_paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    executable_paths.retain(|path| seen.insert(path.to_string_lossy().to_lowercase()));
    executable_paths.sort();
    executable_paths
}

fn terminate_omniscience_processes(executable_paths: &[PathBuf]) -> Result<usize, String> {
    if executable_paths.is_empty() {
        return Ok(0);
    }
    #[cfg(not(windows))]
    {
        let _ = executable_paths;
        Err("Завершение процессов Omniscience поддерживается только в Windows.".to_string())
    }
    #[cfg(windows)]
    {
        let target_paths: Vec<String> = executable_paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let target_paths = serde_json::to_string(&target_paths)
            .map_err(|error| format!("Не удалось подготовить список Omniscience EXE: {error}"))?;
        let script = r#"
$ErrorActionPreference = 'Stop'
$targetSet = @{}
foreach ($target in (ConvertFrom-Json -InputObject $env:OMNISCIENCE_EXECUTABLE_PATHS)) {
    $targetSet[[IO.Path]::GetFullPath([string]$target)] = $true
}
$matchedProcesses = @(Get-CimInstance -ClassName Win32_Process | Where-Object {
    $_.ExecutablePath -and $targetSet.ContainsKey([IO.Path]::GetFullPath($_.ExecutablePath))
})
$processIds = @($matchedProcesses | ForEach-Object { [int]$_.ProcessId })
if ($processIds.Count -gt 0) {
    foreach ($processId in $processIds) {
        $process = Get-Process -Id $processId -ErrorAction SilentlyContinue
        if ($process) { [void]$process.CloseMainWindow() }
    }
    Wait-Process -Id $processIds -Timeout 5 -ErrorAction SilentlyContinue
    $remainingIds = @(Get-CimInstance -ClassName Win32_Process | Where-Object {
        $_.ProcessId -in $processIds -and $_.ExecutablePath -and
            $targetSet.ContainsKey([IO.Path]::GetFullPath($_.ExecutablePath))
    } | ForEach-Object { [int]$_.ProcessId })
    if ($remainingIds.Count -gt 0) {
        Stop-Process -Id $remainingIds -Force -ErrorAction Stop
        Wait-Process -Id $remainingIds -Timeout 10 -ErrorAction SilentlyContinue
    }
}
$remaining = @(Get-CimInstance -ClassName Win32_Process | Where-Object {
    $_.ExecutablePath -and $targetSet.ContainsKey([IO.Path]::GetFullPath($_.ExecutablePath))
})
if ($remaining.Count -gt 0) {
    throw "Some Omniscience processes did not exit: $((@($remaining | ForEach-Object { $_.ProcessId })) -join ', ')"
}
Write-Output $processIds.Count
"#;
        let system_root = env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let powershell = system_root
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let output = Command::new(powershell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .env("OMNISCIENCE_EXECUTABLE_PATHS", target_paths)
            .output()
            .map_err(|error| {
                format!("Не удалось запустить PowerShell для завершения Omniscience: {error}")
            })?;
        if !output.status.success() {
            let details = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if details.is_empty() {
                format!(
                    "PowerShell завершился с кодом {}.",
                    output.status.code().unwrap_or(1)
                )
            } else {
                format!("Не удалось завершить процессы Omniscience: {details}")
            });
        }
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<usize>()
            .map_err(|error| {
                format!("Не удалось прочитать результат завершения Omniscience: {error}")
            })
    }
}

fn stop_all_omniscience_processes_for_state(state: &ManagerState) -> Result<usize, String> {
    let executable_paths = omniscience_executable_paths(state)?;
    terminate_omniscience_processes(&executable_paths)
}

#[tauri::command]
fn launch_omniscience(
    app: AppHandle,
    installer_state: State<'_, InstallerState>,
) -> Result<String, String> {
    let state = load_state(&app);
    let development_directory = if state.mode == "dev" {
        let directory = PathBuf::from(state.source_directory.trim());
        if !directory.join("package.json").is_file() {
            return Err("Dev-режим требует каталог исходников с package.json.".to_string());
        }
        Some(directory)
    } else {
        let executable = PathBuf::from(state.production_executable.trim());
        if !executable.is_file() {
            return Err("Production EXE не настроен или файл отсутствует.".to_string());
        }
        None
    };
    let running = installer_state.running.clone();
    if running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("Подготовка или установка уже выполняется.".to_string());
    }
    let stopped_count = match stop_all_omniscience_processes_for_state(&state) {
        Ok(count) => count,
        Err(error) => {
            running.store(false, Ordering::Release);
            return Err(error);
        }
    };
    if let Some(directory) = development_directory {
        let app_handle = app.clone();
        thread::spawn(move || {
            let result = launch_dev(&app_handle, &directory);
            running.store(false, Ordering::Release);
            let (success, message) = match result {
                Ok(()) => (true, "Dev запущен.".to_string()),
                Err(error) => (false, error),
            };
            let _ = app_handle.emit(
                "dev-setup-finished",
                serde_json::json!({ "success": success, "message": message }),
            );
            if !success {
                emit_output(&app_handle, format!("\nОшибка подготовки Dev: {message}\n"));
            }
        });
        Ok(format!(
            "Завершено предыдущих процессов Omniscience: {stopped_count}. Подготовка Dev запущена; следите за этапами в журнале."
        ))
    } else {
        let executable = PathBuf::from(state.production_executable);
        let launch_result = Command::new(executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Не удалось запустить Prod: {error}"));
        running.store(false, Ordering::Release);
        launch_result?;
        Ok(format!(
            "Завершено предыдущих процессов Omniscience: {stopped_count}. Production-запуск инициирован."
        ))
    }
}

#[tauri::command]
fn stop_all_omniscience(state: State<'_, InstallerState>, app: AppHandle) -> Result<usize, String> {
    if state
        .running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("Подготовка или установка уже выполняется.".to_string());
    }
    let result = stop_all_omniscience_processes_for_state(&load_state(&app));
    state.running.store(false, Ordering::Release);
    result
}

fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn shortcut_compatible_path(path: &Path) -> PathBuf {
    let path = path.to_string_lossy();
    if let Some(unc_path) = path.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc_path}"))
    } else if let Some(dos_path) = path.strip_prefix(r"\\?\") {
        PathBuf::from(dos_path)
    } else {
        PathBuf::from(path.as_ref())
    }
}

fn dev_startup_script(project_directory: &Path) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$logDirectory = Join-Path $env:LOCALAPPDATA 'Omniscience Updater'
$logPath = Join-Path $logDirectory 'dev-startup.log'
        $tauriConfigPath = Join-Path $logDirectory 'dev-tauri-config.json'
        $tauriOutputPath = Join-Path $logDirectory 'dev-tauri.stdout.log'
        $tauriErrorPath = Join-Path $logDirectory 'dev-tauri.stderr.log'
New-Item -ItemType Directory -Path $logDirectory -Force | Out-Null
Start-Transcript -Path $logPath -Append | Out-Null
try {{
    $projectDirectory = {}
    $nodeCandidates = @()
    if ($env:ProgramFiles) {{
        $nodeCandidates += Join-Path $env:ProgramFiles 'nodejs\node.exe'
    }}
    if ($env:LOCALAPPDATA) {{
        $nodeCandidates += Join-Path $env:LOCALAPPDATA 'Programs\nodejs\node.exe'
    }}
    $pathNode = Get-Command 'node.exe' -ErrorAction SilentlyContinue
    if ($pathNode) {{
        $nodeCandidates += $pathNode.Source
    }}
    $node = $nodeCandidates |
        Where-Object {{ $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) }} |
        Select-Object -First 1
    if (-not $node) {{
        throw 'Node.js was not found. Open the updater and prepare Dev mode first.'
    }}
    $nodeDirectory = Split-Path -Parent $node
    $npmCli = Join-Path $nodeDirectory 'node_modules\npm\bin\npm-cli.js'
    if (-not (Test-Path -LiteralPath $npmCli -PathType Leaf)) {{
        throw "npm-cli.js was not found at $npmCli"
    }}
    $cargoDirectory = Join-Path $env:USERPROFILE '.cargo\bin'
    $env:PATH = "$nodeDirectory;$cargoDirectory;$env:PATH"
    $env:CARGO_TERM_COLOR = 'never'
    $env:CARGO_TERM_PROGRESS_WHEN = 'never'
    Set-Location -LiteralPath $projectDirectory

    Write-Output '[startup] Installing npm dependencies.'
    & $node $npmCli install --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) {{
        throw "npm install failed with exit code $LASTEXITCODE"
    }}

    $tauriCli = Join-Path $projectDirectory 'node_modules\@tauri-apps\cli\tauri.js'
    if (-not (Test-Path -LiteralPath $tauriCli -PathType Leaf)) {{
        throw "Tauri CLI was not found at $tauriCli"
    }}
    $portListener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $portListener.Start()
    $port = $portListener.LocalEndpoint.Port
    $portListener.Stop()
    $tauriConfig = @{{
        build = @{{
            devUrl = "http://127.0.0.1:$port"
            beforeDevCommand = "npm run build:project && npm run preview -- --host 127.0.0.1 --port $port --strictPort"
        }}
    }} | ConvertTo-Json -Depth 4 -Compress
    [System.IO.File]::WriteAllText(
        $tauriConfigPath,
        $tauriConfig,
        [System.Text.UTF8Encoding]::new($false)
    )
    Remove-Item Env:\TAURI_CONFIG -ErrorAction SilentlyContinue

    Write-Output "[startup] Starting Omniscience Dev on port $port."
    Write-Output "[startup] Tauri logs: $tauriOutputPath and $tauriErrorPath"
    Remove-Item -LiteralPath $tauriOutputPath, $tauriErrorPath -Force -ErrorAction SilentlyContinue
    $tauriArguments = '"{{0}}" dev --config "{{1}}"' -f $tauriCli, $tauriConfigPath
    $tauriProcess = Start-Process `
        -FilePath $node `
        -ArgumentList $tauriArguments `
        -WorkingDirectory $projectDirectory `
        -WindowStyle Hidden `
        -RedirectStandardOutput $tauriOutputPath `
        -RedirectStandardError $tauriErrorPath `
        -PassThru
    Write-Output "[startup] Tauri CLI launched as PID $($tauriProcess.Id); inspect its log files for the result."
}} catch {{
    Write-Error $_
    exit 1
}} finally {{
    Stop-Transcript | Out-Null
}}
"#,
        powershell_literal(&project_directory.to_string_lossy())
    )
}

#[tauri::command]
fn add_omniscience_to_startup(app: AppHandle) -> Result<String, String> {
    let state = load_state(&app);
    let (target, arguments, working_directory, startup_script) = match state.mode.as_str() {
        "dev" => {
            let project_directory = PathBuf::from(state.source_directory.trim());
            if !project_directory.join("package.json").is_file() {
                return Err(
                    "Для автозагрузки Dev укажите существующий каталог исходников.".to_string(),
                );
            }
            let script_path = app
                .path()
                .app_data_dir()
                .map_err(|error| {
                    format!("Не удалось определить каталог данных приложения: {error}")
                })?
                .join("omniscience-startup.ps1");
            let script = dev_startup_script(&project_directory);
            fs::write(&script_path, script).map_err(|error| {
                format!("Не удалось сохранить Dev-скрипт автозагрузки: {error}")
            })?;

            let powershell = env::var_os("SystemRoot")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe");
            let arguments = format!(
                "-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File \"{}\"",
                script_path.display()
            );
            (powershell, arguments, project_directory, Some(script_path))
        }
        "prod" => {
            let executable = PathBuf::from(state.production_executable.trim());
            if !executable.is_file() {
                return Err("Production EXE не настроен или файл отсутствует.".to_string());
            }
            let executable = executable
                .canonicalize()
                .map_err(|error| format!("Не удалось определить путь к Production EXE: {error}"))?;
            let working_directory = executable
                .parent()
                .ok_or_else(|| "Не удалось определить каталог Production EXE.".to_string())?
                .to_path_buf();
            (executable, String::new(), working_directory, None)
        }
        _ => return Err("Выберите режим Dev или Prod перед настройкой автозагрузки.".to_string()),
    };

    #[cfg(not(windows))]
    {
        let _ = (target, arguments, working_directory, startup_script);
        return Err("Настройка автозагрузки поддерживается только в Windows.".to_string());
    }

    #[cfg(windows)]
    {
        let target = shortcut_compatible_path(&target);
        let working_directory = shortcut_compatible_path(&working_directory);
        let script = r#"
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [Console]::OutputEncoding
$ErrorActionPreference = 'Stop'
$startupDirectory = [Environment]::GetFolderPath('Startup')
if (-not $startupDirectory) { throw 'Could not resolve the current user Startup folder.' }
$shortcutPath = Join-Path $startupDirectory 'The_Omniscience.lnk'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $env:OMNISCIENCE_STARTUP_TARGET
$shortcut.Arguments = $env:OMNISCIENCE_STARTUP_ARGUMENTS
$shortcut.WorkingDirectory = $env:OMNISCIENCE_STARTUP_WORKING_DIRECTORY
$shortcut.Description = 'Start the selected Omniscience version when you sign in.'
$shortcut.WindowStyle = 7
$shortcut.Save()
Write-Output $shortcutPath
"#;
        let system_root = env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let powershell = system_root
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let output = Command::new(powershell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .env("OMNISCIENCE_STARTUP_TARGET", &target)
            .env("OMNISCIENCE_STARTUP_ARGUMENTS", &arguments)
            .env("OMNISCIENCE_STARTUP_WORKING_DIRECTORY", &working_directory)
            .output()
            .map_err(|error| format!("Не удалось создать ярлык автозагрузки: {error}"))?;
        if !output.status.success() {
            let details = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if details.is_empty() {
                format!(
                    "PowerShell завершился с кодом {} при создании ярлыка автозагрузки.",
                    output.status.code().unwrap_or(1)
                )
            } else {
                format!("Не удалось создать ярлык автозагрузки: {details}")
            });
        }

        if let Some(script_path) = startup_script {
            emit_output(
                &app,
                format!(
                    "Dev-автозагрузка настроена; журнал запуска будет записываться в {}.\n",
                    script_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join("dev-startup.log")
                        .display()
                ),
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
        return Err(format!(
            "PTY host script not found: {}",
            host_script.display()
        ));
    }
    let installer_script = base_directory.join("scripts").join(script_name);
    if !installer_script.is_file() {
        return Err(format!(
            "Installer script not found: {}",
            installer_script.display()
        ));
    }
    if !cfg!(debug_assertions) && !node_executable.is_file() {
        return Err(format!(
            "Bundled Node runtime not found: {}",
            node_executable.display()
        ));
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
            return Err(format!(
                "Could not start the bundled installer host: {error}"
            ));
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
            get_output_log_path,
            set_manager_state,
            list_installed_versions,
            check_prerequisites,
            inspect_source,
            prepare_release,
            check_latest_release,
            launch_omniscience,
            stop_all_omniscience,
            add_omniscience_to_startup,
            open_github_browser,
            navigate_github_browser,
            open_github_devtools,
            is_github_artifact_installed,
            install_github_artifact
        ])
        .run(tauri::generate_context!())
        .expect("error while running Omniscience Updater");
}

#[cfg(test)]
mod tests {
    use super::{
        ManagerState, dev_startup_script, node_is_compatible, omniscience_executable_paths_in_root,
        scan_installed_versions, shortcut_compatible_path, tauri_dev_config,
    };
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "omniscience-updater-test-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn scan_lists_runnable_dev_and_exe_installations() {
        let root = TestRoot::new();
        let dev = root.0.join("owner").join("repo").join("v1").join("dev");
        fs::create_dir_all(dev.join(".git")).unwrap();
        fs::write(dev.join("package.json"), "{}").unwrap();

        let prod = root.0.join("owner").join("repo").join("v2").join("prod");
        fs::create_dir_all(&prod).unwrap();
        fs::write(prod.join("app.exe"), b"").unwrap();
        fs::write(prod.join("readme.txt"), b"").unwrap();

        let dev_versions = scan_installed_versions(&root.0, "dev").unwrap();
        assert_eq!(dev_versions.len(), 1);
        assert_eq!(dev_versions[0].tag, "v1");
        assert_eq!(dev_versions[0].path, dev.to_string_lossy().to_string());

        let prod_versions = scan_installed_versions(&root.0, "prod").unwrap();
        assert_eq!(prod_versions.len(), 1);
        assert_eq!(prod_versions[0].tag, "v2");
        assert_eq!(
            prod_versions[0].path,
            prod.join("app.exe").to_string_lossy().to_string()
        );
    }

    #[test]
    fn collects_all_installed_prod_and_dev_executables() {
        let root = TestRoot::new();
        let installation_root = root.0.join("The_Omniscience");
        let prod = installation_root
            .join("owner")
            .join("repo")
            .join("v1")
            .join("prod")
            .join("Omniscience.exe");
        fs::create_dir_all(prod.parent().unwrap()).unwrap();
        fs::write(&prod, b"").unwrap();

        let dev = installation_root
            .join("owner")
            .join("repo")
            .join("v2")
            .join("dev");
        fs::create_dir_all(dev.join(".git")).unwrap();
        fs::write(dev.join("package.json"), "{}").unwrap();
        let dev_executable = dev
            .join("src-tauri")
            .join("target")
            .join("debug")
            .join("The_Omniscience.exe");
        fs::create_dir_all(dev_executable.parent().unwrap()).unwrap();
        fs::write(&dev_executable, b"").unwrap();

        let state = ManagerState {
            mode: "prod".to_string(),
            source_directory: String::new(),
            production_executable: String::new(),
            drive_letter: "C".to_string(),
            browser_url: String::new(),
        };
        let executables = omniscience_executable_paths_in_root(&state, &installation_root).unwrap();
        assert!(executables.contains(&prod));
        assert!(executables.contains(&dev_executable));
    }

    #[test]
    fn dev_startup_script_passes_dynamic_config_to_tauri_cli() {
        let script = dev_startup_script(Path::new(r"C:\Users\O'Brien\Omniscience"));
        assert!(script.contains("'C:\\Users\\O''Brien\\Omniscience'"));
        assert!(script.contains("-FilePath $node"));
        assert!(script.contains("-ArgumentList $tauriArguments"));
        assert!(script.contains("-RedirectStandardOutput $tauriOutputPath"));
        assert!(script.contains("-RedirectStandardError $tauriErrorPath"));
        assert!(script.contains("Tauri CLI launched as PID"));
        assert!(script.contains("System.Net.Sockets.TcpListener"));
        assert!(script.contains("npm run preview -- --host 127.0.0.1 --port $port --strictPort"));
        assert!(!script.contains("CARGO_BUILD_JOBS"));
        assert!(!script.contains("CARGO_INCREMENTAL"));
        assert!(script.contains("[System.IO.File]::WriteAllText("));
        assert!(script.contains("[System.Text.UTF8Encoding]::new($false)"));
        assert!(script.contains("[startup] Starting Omniscience Dev on port $port"));
        assert!(script.contains("dev-tauri.stdout.log"));
        assert!(script.contains("dev-tauri.stderr.log"));
        assert!(script.contains("dev-startup.log"));
    }

    #[test]
    fn startup_shortcut_paths_do_not_use_extended_windows_prefixes() {
        assert_eq!(
            shortcut_compatible_path(Path::new(r"\\?\C:\Omniscience\app.exe")),
            PathBuf::from(r"C:\Omniscience\app.exe")
        );
        assert_eq!(
            shortcut_compatible_path(Path::new(r"\\?\UNC\server\share\app.exe")),
            PathBuf::from(r"\\server\share\app.exe")
        );
    }

    #[test]
    fn node_version_check_matches_vite_8_requirements() {
        assert!(node_is_compatible("v20.19.0"));
        assert!(node_is_compatible("v22.12.0"));
        assert!(node_is_compatible("v24.0.0"));
        assert!(!node_is_compatible("v20.18.1"));
        assert!(!node_is_compatible("v22.11.0"));
        assert!(!node_is_compatible("v21.7.0"));
        assert!(!node_is_compatible("unknown"));
    }

    #[test]
    fn tauri_dev_config_uses_the_same_dynamic_preview_port() {
        let config = tauri_dev_config(23456);
        assert_eq!(config["build"]["devUrl"], "http://127.0.0.1:23456");
        assert_eq!(
            config["build"]["beforeDevCommand"],
            "npm run build:project && npm run preview -- --host 127.0.0.1 --port 23456 --strictPort"
        );
    }
}
