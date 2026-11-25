import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { compareReleaseTags } from './version-sort.js';
import './styles.css';

const log = document.querySelector('#log');
const status = document.querySelector('#status');
const hint = document.querySelector('#log-hint');
const installerButtons = [...document.querySelectorAll('button[data-action]')];
const stopOmniscienceButton = document.querySelector('#stop-omniscience');
const startupButton = document.querySelector('#add-omniscience-startup');
const logQueue = [];
const logFlushIntervalMs = 50;
const logFlushCharLimit = 65536;
const logTextNodeLimit = 8192;
const logVisibleCharLimit = 500000;
const logStyle = {
  foreground: '',
  background: '',
  bold: false,
  dim: false,
  italic: false,
  underline: false,
};
let active = false;
let managerState;
let logQueueHead = 0;
let logQueueOffset = 0;
let logFlushScheduled = false;
let activeLogNode;
let activeLogNodeLength = 0;
let activeLogNodeStyle = '';
let logEntries = [];
let logRenderedLength = 0;
let followLogOutput = true;

function setBusy(isBusy, label = 'Готово') {
  active = isBusy;
  for (const button of installerButtons) button.disabled = isBusy;
  stopOmniscienceButton.disabled = isBusy;
  startupButton.disabled = isBusy;
  document.querySelector('#launch').disabled = isBusy;
  status.textContent = isBusy ? 'Выполняется…' : label;
  status.dataset.state = isBusy ? 'busy' : label === 'Готово' ? 'idle' : 'error';
}

function appendOutput(text) {
  const output = String(text).replace(/\r\n?/g, '\n');
  if (!output) return;
  logQueue.push(output);
  scheduleLogFlush();
}

function scrollToLog() {
  document.querySelector('.log-section').scrollIntoView({
    behavior: 'smooth',
    block: 'start',
  });
}

function scheduleLogFlush() {
  if (logFlushScheduled) return;
  logFlushScheduled = true;
  setTimeout(flushLogOutput, logFlushIntervalMs);
}

function takeLogBatch(maxLength) {
  const chunks = [];
  let length = 0;
  while (logQueueHead < logQueue.length && length < maxLength) {
    const queued = logQueue[logQueueHead];
    const available = queued.length - logQueueOffset;
    let take = Math.min(available, maxLength - length);
    const end = logQueueOffset + take;
    if (
      take < available
      && /[\uD800-\uDBFF]/.test(queued[end - 1])
      && /[\uDC00-\uDFFF]/.test(queued[end])
    ) {
      take += 1;
    }
    chunks.push(queued.slice(logQueueOffset, logQueueOffset + take));
    length += take;
    logQueueOffset += take;
    if (logQueueOffset === queued.length) {
      logQueueHead += 1;
      logQueueOffset = 0;
    }
  }
  if (logQueueHead === logQueue.length) {
    logQueue.length = 0;
    logQueueHead = 0;
  } else if (logQueueHead > 256) {
    logQueue.splice(0, logQueueHead);
    logQueueHead = 0;
  }
  return chunks.join('');
}

function currentLogStyle() {
  return [
    logStyle.foreground,
    logStyle.background,
    logStyle.bold ? 'ansi-bold' : '',
    logStyle.dim ? 'ansi-dim' : '',
    logStyle.italic ? 'ansi-italic' : '',
    logStyle.underline ? 'ansi-underline' : '',
  ].filter(Boolean).join(' ');
}

function resetLogStyle() {
  logStyle.foreground = '';
  logStyle.background = '';
  logStyle.bold = false;
  logStyle.dim = false;
  logStyle.italic = false;
  logStyle.underline = false;
}

function applyAnsiStyle(parameters) {
  const codes = parameters === '' ? [0] : parameters.split(';').map((code) => Number(code || 0));
  for (let index = 0; index < codes.length; index += 1) {
    const code = codes[index];
    if (code === 0) resetLogStyle();
    else if (code === 1) logStyle.bold = true;
    else if (code === 2) logStyle.dim = true;
    else if (code === 3) logStyle.italic = true;
    else if (code === 4) logStyle.underline = true;
    else if (code === 22) {
      logStyle.bold = false;
      logStyle.dim = false;
    } else if (code === 23) logStyle.italic = false;
    else if (code === 24) logStyle.underline = false;
    else if (code === 39) logStyle.foreground = '';
    else if (code === 49) logStyle.background = '';
    else if ((code >= 30 && code <= 37) || (code >= 90 && code <= 97)) {
      logStyle.foreground = `ansi-fg-${code}`;
    } else if ((code >= 40 && code <= 47) || (code >= 100 && code <= 107)) {
      logStyle.background = `ansi-bg-${code}`;
    }
  }
}

function appendStyledLogText(text, style) {
  let offset = 0;
  while (offset < text.length) {
    if (!activeLogNode || activeLogNodeLength >= logTextNodeLimit || activeLogNodeStyle !== style) {
      activeLogNode = style ? document.createElement('span') : document.createTextNode('');
      if (style) activeLogNode.className = style;
      activeLogNodeStyle = style;
      activeLogNodeLength = 0;
      log.append(activeLogNode);
      logEntries.push({ node: activeLogNode, length: 0 });
    }
    let end = Math.min(offset + logTextNodeLimit - activeLogNodeLength, text.length);
    if (end < text.length && /[\uD800-\uDBFF]/.test(text[end - 1]) && /[\uDC00-\uDFFF]/.test(text[end])) {
      if (end - offset === 1 && activeLogNodeLength > 0) {
        activeLogNode = undefined;
        continue;
      }
      end -= 1;
    }
    const part = text.slice(offset, end);
    if (activeLogNode.nodeType === Node.TEXT_NODE) activeLogNode.appendData(part);
    else activeLogNode.append(document.createTextNode(part));
    activeLogNodeLength += part.length;
    logRenderedLength += part.length;
    logEntries[logEntries.length - 1].length += part.length;
    offset = end;
  }
}

function trimVisibleLog() {
  while (logRenderedLength > logVisibleCharLimit && logEntries.length > 1) {
    const oldest = logEntries.shift();
    oldest.node.remove();
    logRenderedLength -= oldest.length;
  }
}

function appendLogText(text) {
  const ansi = /\u001b\[([0-9;]*)m|\u001b\[[0-?]*[ -/]*[@-~]|\u001b\][^\u0007]*(?:\u0007|\u001b\\)/g;
  let offset = 0;
  let match;
  while ((match = ansi.exec(text)) !== null) {
    if (match.index > offset) {
      appendStyledLogText(text.slice(offset, match.index), currentLogStyle());
    }
    if (match[1] !== undefined) applyAnsiStyle(match[1]);
    offset = ansi.lastIndex;
  }
  if (offset < text.length) appendStyledLogText(text.slice(offset), currentLogStyle());
  trimVisibleLog();
}

function flushLogOutput() {
  const batch = takeLogBatch(logFlushCharLimit);
  if (batch) {
    appendLogText(batch);
    if (followLogOutput) log.scrollTop = log.scrollHeight;
  }
  if (logQueueHead < logQueue.length) {
    setTimeout(flushLogOutput, logFlushIntervalMs);
  } else {
    logFlushScheduled = false;
  }
}

function clearLogOutput() {
  logQueue.length = 0;
  logQueueHead = 0;
  logQueueOffset = 0;
  log.replaceChildren();
  activeLogNode = undefined;
  activeLogNodeLength = 0;
  activeLogNodeStyle = '';
  logEntries = [];
  logRenderedLength = 0;
  resetLogStyle();
  followLogOutput = true;
}

log.addEventListener('scroll', () => {
  followLogOutput = log.scrollHeight - log.scrollTop - log.clientHeight < 16;
});

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
  startupButton.disabled = active || !selected || selected.dataset.mode !== mode;
  const summary = document.querySelector('#launch-summary');
  if (!selected || selected.dataset.mode !== mode) {
    summary.textContent = `${mode.toUpperCase()} · выберите установленную версию`;
    launch.disabled = true;
    return;
  }

  summary.textContent = `${mode.toUpperCase()} · ${selected.dataset.title} · ${selected.value}`;
  launch.disabled = active;
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
    versions.sort((left, right) => (
      compareReleaseTags(right.tag, left.tag)
      || left.owner.localeCompare(right.owner)
      || left.repo.localeCompare(right.repo)
      || left.path.localeCompare(right.path)
    ));
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

await listen('dev-setup-finished', async (event) => {
  const { success, message } = event.payload;
  setBusy(false, success ? 'Dev запущен' : 'Ошибка Dev');
  hint.textContent = success
    ? 'Подготовка завершена; вывод Dev появится в журнале'
    : 'Проверьте журнал и установку требуемых инструментов';
  appendOutput(`\n${message}\n`);
  if (success) await checkPrerequisites();
});

await listen('dev-process-finished', (event) => {
  const { success, exitCode, error } = event.payload;
  const label = success ? 'Dev завершён' : `Ошибка Dev (${exitCode})`;
  status.textContent = label;
  status.dataset.state = success ? 'idle' : 'error';
  appendOutput(error
    ? `\nПроцесс Dev не смог завершиться корректно: ${error}\n`
    : `\nПроцесс Dev завершился с кодом ${exitCode}.\n`);
});

for (const button of installerButtons) {
  button.addEventListener('click', async () => {
    if (active) return;
    const action = button.dataset.action;
    clearLogOutput();
  scrollToLog();
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
  scrollToLog();
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
  scrollToLog();
  try {
    await invoke('open_github_devtools');
  } catch (error) {
    appendOutput(`\nНе удалось открыть DevTools: ${String(error)}\n`);
  }
});

document.querySelector('#launch').addEventListener('click', async () => {
  const isDev = selectedMode() === 'dev';
  scrollToLog();
  setBusy(true, isDev ? 'Подготовка Dev' : 'Запуск Omniscience');
  try {
    if (!document.querySelector('#installed-versions input[name="installed-version"]:checked')) {
      throw new Error('Сначала выберите установленную версию.');
    }
    await saveState();
    const message = await invoke('launch_omniscience');
    appendOutput(`\n${message}\n`);
    if (!isDev) setBusy(false);
  } catch (error) {
    setBusy(false, 'Ошибка запуска');
    appendOutput(`\nНе удалось запустить: ${String(error)}\n`);
  }
});

stopOmniscienceButton.addEventListener('click', async () => {
  if (active) return;
  scrollToLog();
  setBusy(true, 'Завершение Omniscience');
  try {
    const stoppedCount = await invoke('stop_all_omniscience');
    appendOutput(
      stoppedCount > 0
        ? `\nЗавершено процессов Omniscience: ${stoppedCount}.\n`
        : '\nЗапущенные процессы Omniscience не найдены.\n',
    );
    setBusy(false);
  } catch (error) {
    setBusy(false, 'Ошибка завершения');
    appendOutput(`\nНе удалось завершить Omniscience: ${String(error)}\n`);
  }
});

startupButton.addEventListener('click', async () => {
  if (active) return;
  scrollToLog();
  setBusy(true, 'Добавление в автозагрузку');
  try {
    await saveState();
    const message = await invoke('add_omniscience_to_startup');
    appendOutput(`\n${message}\n`);
    setBusy(false);
  } catch (error) {
    setBusy(false, 'Ошибка автозагрузки');
    appendOutput(`\nНе удалось добавить Omniscience в автозагрузку: ${String(error)}\n`);
  }
});

managerState = await getState();
try {
  const outputLogPath = await invoke('get_output_log_path');
  document.querySelector('#log-file-path').textContent =
    `Полный вывод сохраняется в ${outputLogPath}; здесь показана последняя часть журнала.`;
  document.querySelector('#log-file-path').title = outputLogPath;
} catch (error) {
  document.querySelector('#log-file-path').textContent =
    `Не удалось получить путь к полному журналу: ${String(error)}`;
}
document.querySelector(`input[name="mode"][value="${managerState.mode === 'dev' ? 'dev' : 'prod'}"]`).checked = true;
document.querySelector('#drive-letter').value = managerState.driveLetter || 'C';
document.querySelector('#github-url').value = managerState.browserUrl || 'https://github.com/EssenceOfDream/The_Omniscience/releases';
updateInstallPathPreview();

await loadInstalledVersions();
await checkPrerequisites();
