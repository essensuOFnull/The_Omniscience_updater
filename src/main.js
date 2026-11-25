import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './styles.css';

const log = document.querySelector('#log');
const status = document.querySelector('#status');
const hint = document.querySelector('#log-hint');
const installerButtons = [...document.querySelectorAll('button[data-action]')];
const ansiEscape = /\u001b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])/g;
let active = false;
let managerState;

function setBusy(isBusy, label = 'Готово') {
  active = isBusy;
  for (const button of installerButtons) button.disabled = isBusy;
  status.textContent = isBusy ? 'Выполняется…' : label;
  status.dataset.state = isBusy ? 'busy' : label === 'Готово' ? 'idle' : 'error';
}

function appendOutput(text) {
  const cleanText = String(text).replace(ansiEscape, '').replace(/\r\n?/g, '\n');
  log.textContent += cleanText;
  if (log.textContent.length > 200000) {
    log.textContent = log.textContent.slice(-180000);
  }
  log.scrollTop = log.scrollHeight;
}

async function getState() {
  return invoke('get_manager_state');
}

function selectedMode() {
  return document.querySelector('input[name="mode"]:checked')?.value || managerState?.mode || 'prod';
}

function updateLaunchSummary() {
  const mode = selectedMode();
  const selected = document.querySelector('#installed-versions input[name="installed-version"]:checked');
  const launch = document.querySelector('#launch');
  const summary = document.querySelector('#launch-summary');
  if (!selected || selected.dataset.mode !== mode) {
    summary.textContent = `${mode.toUpperCase()} · выберите установленную версию`;
    launch.disabled = true;
    return;
  }

  summary.textContent = `${mode.toUpperCase()} · ${selected.dataset.title} · ${selected.value}`;
  launch.disabled = false;
}

async function persistManagerState(updates = {}) {
  managerState = await invoke('set_manager_state', {
    mode: selectedMode(),
    sourceDirectory: managerState?.sourceDirectory || '',
    productionExecutable: managerState?.productionExecutable || '',
    driveLetter: document.querySelector('#drive-letter').value,
    browserUrl: document.querySelector('#github-url').value.trim(),
    ...updates,
  });
  updateLaunchSummary();
  return managerState;
}

async function saveState() {
  const mode = selectedMode();
  const selected = document.querySelector('#installed-versions input[name="installed-version"]:checked');
  const updates = {};
  if (selected?.dataset.mode === mode) {
    if (mode === 'dev') updates.sourceDirectory = selected.value;
    else updates.productionExecutable = selected.value;
  }
  await persistManagerState(updates);
}

function renderInstalledVersion(version, mode, selectedPath) {
  const label = document.createElement('label');
  label.className = 'installed-version';

  const radio = document.createElement('input');
  radio.type = 'radio';
  radio.name = 'installed-version';
  radio.value = version.path;
  radio.dataset.mode = mode;
  radio.dataset.title = `${version.owner}/${version.repo} · ${version.tag}`;
  radio.checked = version.path === selectedPath;

  const copy = document.createElement('span');
  copy.className = 'installed-version-copy';
  const title = document.createElement('span');
  title.className = 'installed-version-title';
  title.textContent = radio.dataset.title;
  const path = document.createElement('span');
  path.className = 'installed-version-path';
  path.textContent = version.path;
  copy.append(title, path);
  label.append(radio, copy);

  radio.addEventListener('change', async () => {
    const update = mode === 'dev'
      ? { sourceDirectory: version.path }
      : { productionExecutable: version.path };
    try {
      await persistManagerState(update);
    } catch (error) {
      appendOutput(`\nНе удалось выбрать версию: ${String(error)}\n`);
      await loadInstalledVersions();
    }
  });
  return label;
}

async function loadInstalledVersions() {
  const mode = selectedMode();
  const container = document.querySelector('#installed-versions');
  container.replaceChildren();
  const loading = document.createElement('p');
  loading.className = 'inline-status';
  loading.textContent = 'Загрузка списка…';
  container.append(loading);

  try {
    const versions = await invoke('list_installed_versions', { mode });
    container.replaceChildren();
    if (versions.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'inline-status';
      empty.textContent = `На диске ${document.querySelector('#drive-letter').value} установленные версии ${mode.toUpperCase()} не найдены.`;
      container.append(empty);
      updateLaunchSummary();
      return;
    }

    const selectedPath = mode === 'dev'
      ? managerState.sourceDirectory
      : managerState.productionExecutable;
    for (const version of versions) {
      container.append(renderInstalledVersion(version, mode, selectedPath));
    }
    updateLaunchSummary();
  } catch (error) {
    container.replaceChildren();
    const message = document.createElement('p');
    message.className = 'inline-status';
    message.dataset.state = 'error';
    message.textContent = `Не удалось загрузить список: ${String(error)}`;
    container.append(message);
    updateLaunchSummary();
  }
}

function renderPrerequisites(report) {
  const container = document.querySelector('#prerequisites');
  container.replaceChildren();
  for (const check of report.Checks || []) {
    const item = document.createElement('div');
    item.className = 'prerequisite';
    item.dataset.status = String(check.Status).toLowerCase();
    const name = document.createElement('strong');
    name.textContent = check.Name;
    const state = document.createElement('span');
    state.textContent = [check.Status, check.Version].filter(Boolean).join(' · ');
    item.append(name, state);
    container.append(item);
  }
}

async function checkPrerequisites() {
  const container = document.querySelector('#prerequisites');
  container.textContent = 'Запуск проверки…';
  try {
    const report = await invoke('check_prerequisites');
    renderPrerequisites(report);
  } catch (error) {
    container.textContent = `Ошибка: ${String(error)}`;
  }
}

async function checkLatestRelease() {
  const target = document.querySelector('#release-status');
  target.textContent = 'Запрос GitHub Releases…';
  try {
    const release = await invoke('check_latest_release');
    if (!release.available) {
      target.textContent = release.error || 'Релиз недоступен.';
      target.dataset.state = 'error';
      return;
    }
    target.dataset.state = 'ready';
    target.textContent = `${release.name || release.tag || 'Новый релиз'}${release.publishedAt ? ` · ${release.publishedAt}` : ''}`;
    if (release.tag) document.querySelector('#release-tag').value = release.tag;
  } catch (error) {
    target.textContent = `Ошибка проверки: ${String(error)}`;
    target.dataset.state = 'error';
  }
}

function updateInstallPathPreview() {
  const drive = document.querySelector('#drive-letter').value;
  const preview = document.querySelector('#install-path-preview');
  preview.textContent = `Каталог: ${drive}:\\The_Omniscience\\${ownerAndRepoPlaceholder()}\\<tag>\\<prod|dev>`;
}

function ownerAndRepoPlaceholder() {
  return '<owner>\\<repository>';
}

await listen('installer-output', (event) => appendOutput(event.payload.text));

await listen('github-browser-load', (event) => {
  const { event: loadEvent, url } = event.payload;
  appendOutput(`\nGitHub WebView ${loadEvent}: ${url}\n`);
});

await listen('installed-version-changed', async () => {
  try {
    managerState = await getState();
    document.querySelector(`input[name="mode"][value="${managerState.mode}"]`).checked = true;
    await loadInstalledVersions();
  } catch (error) {
    appendOutput(`\nНе удалось обновить список установок: ${String(error)}\n`);
  }
});

await listen('installer-finished', (event) => {
  const { success, exitCode } = event.payload;
  setBusy(false, success ? 'Установка завершена' : `Ошибка (код ${exitCode})`);
  hint.textContent = success ? 'Процесс PowerShell завершён' : 'Проверьте журнал и ограничения Windows';
  appendOutput(success
    ? '\n\nОперация завершена.\n'
    : `\n\nОперация завершилась с ошибкой (код ${exitCode}).\n`);
});

for (const button of installerButtons) {
  button.addEventListener('click', async () => {
    if (active) return;
    const action = button.dataset.action;
    log.textContent = '';
    hint.textContent = action === 'winget'
      ? 'Подготовка App Installer'
      : 'Подготовка Microsoft Store';
    setBusy(true);
    appendOutput('Запуск PowerShell…\n');
    try {
      await invoke('start_install', { action });
    } catch (error) {
      setBusy(false, 'Не удалось запустить');
      hint.textContent = 'Проверьте файлы приложения и журнал ошибки';
      appendOutput(`\nОшибка запуска: ${String(error)}\n`);
    }
  });
}

document.querySelector('#check-prerequisites').addEventListener('click', checkPrerequisites);
document.querySelector('#check-release').addEventListener('click', checkLatestRelease);
for (const radio of document.querySelectorAll('input[name="mode"]')) {
  radio.addEventListener('change', async () => {
    try {
      await persistManagerState({ mode: radio.value });
      await loadInstalledVersions();
    } catch (error) {
      appendOutput(`\nНе удалось изменить режим: ${String(error)}\n`);
    }
  });
}
document.querySelector('#drive-letter').addEventListener('change', async () => {
  updateInstallPathPreview();
  try {
    await saveState();
    await loadInstalledVersions();
  } catch (error) {
    appendOutput(`\nНе удалось сохранить диск установки: ${String(error)}\n`);
  }
});
document.querySelector('#open-github').addEventListener('click', async () => {
  const url = document.querySelector('#github-url').value.trim();
  appendOutput(`\nОткрытие GitHub: ${url || '(адрес не указан)'}\n`);
  try {
    await saveState();
    const opened = await invoke('open_github_browser', { url });
    document.querySelector('#github-url').value = opened;
  } catch (error) {
    appendOutput(`\nНе удалось открыть браузер: ${String(error)}\n`);
  }
});
document.querySelector('#open-github-devtools').addEventListener('click', async () => {
  try {
    await invoke('open_github_devtools');
  } catch (error) {
    appendOutput(`\nНе удалось открыть DevTools: ${String(error)}\n`);
  }
});

document.querySelector('#prepare-release').addEventListener('click', async () => {
  const tag = document.querySelector('#release-tag').value.trim();
  appendOutput(`\nПодготовка release ${tag || '(tag не указан)'}…\n`);
  setBusy(true, 'Подготовка release');
  try {
    await saveState();
    const result = await invoke('prepare_release', { tag });
    appendOutput(`Release подготовлен.\ncommit ${result.commit || 'не определён'}\n`);
    setBusy(false, 'Release готов');
  } catch (error) {
    appendOutput(`\nОшибка подготовки: ${String(error)}\n`);
    setBusy(false, 'Ошибка release');
  }
});

document.querySelector('#launch').addEventListener('click', async () => {
  try {
    if (!document.querySelector('#installed-versions input[name="installed-version"]:checked')) {
      throw new Error('Сначала выберите установленную версию.');
    }
    await saveState();
    const message = await invoke('launch_omniscience');
    appendOutput(`\n${message}\n`);
  } catch (error) {
    appendOutput(`\nНе удалось запустить: ${String(error)}\n`);
  }
});

managerState = await getState();
document.querySelector(`input[name="mode"][value="${managerState.mode === 'dev' ? 'dev' : 'prod'}"]`).checked = true;
document.querySelector('#drive-letter').value = managerState.driveLetter || 'C';
document.querySelector('#github-url').value = managerState.browserUrl || 'https://github.com/EssenceOfDream/The_Omniscience/releases';
updateInstallPathPreview();

await loadInstalledVersions();
await checkPrerequisites();
await checkLatestRelease();
