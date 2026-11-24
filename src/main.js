import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './styles.css';

const log = document.querySelector('#log');
const status = document.querySelector('#status');
const hint = document.querySelector('#log-hint');
const installerButtons = [...document.querySelectorAll('button[data-action]')];
const ansiEscape = /\u001b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])/g;
let active = false;

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

async function inspectSource() {
  const sourceDirectory = document.querySelector('#source-directory').value.trim();
  const sourceStatus = document.querySelector('#source-status');
  sourceStatus.textContent = 'Проверка рабочей копии…';
  try {
    const result = await invoke('inspect_source', { sourceDirectory });
    const details = [];
    if (result.commit) details.push(`commit ${result.commit.slice(0, 12)}`);
    if (result.branch) details.push(`ветка ${result.branch}`);
    if (result.tag) details.push(`tag ${result.tag}`);
    sourceStatus.textContent = [result.status, ...details].filter(Boolean).join(' · ');
  } catch (error) {
    sourceStatus.textContent = `Ошибка проверки: ${String(error)}`;
  }
}

async function saveState() {
  const mode = document.querySelector('input[name="mode"]:checked')?.value || 'prod';
  const sourceDirectory = document.querySelector('#source-directory').value.trim();
  const productionExecutable = document.querySelector('#production-executable').value.trim();
  const driveLetter = document.querySelector('#drive-letter').value;
  const browserUrl = document.querySelector('#github-url').value.trim();
  try {
    const state = await invoke('set_manager_state', {
      mode,
      sourceDirectory,
      productionExecutable,
      driveLetter,
      browserUrl,
    });
    document.querySelector('#launch-summary').textContent =
      state.mode === 'dev'
        ? `Dev · ${state.sourceDirectory || 'каталог не задан'}`
        : `Prod · ${state.productionExecutable || 'EXE не задан'}`;
    await inspectSource();
  } catch (error) {
    setBusy(false, 'Ошибка настроек');
    appendOutput(`\nНе удалось сохранить настройки: ${String(error)}\n`);
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

document.querySelector('#save-state').addEventListener('click', saveState);
document.querySelector('#check-prerequisites').addEventListener('click', checkPrerequisites);
document.querySelector('#check-release').addEventListener('click', checkLatestRelease);
document.querySelector('#source-directory').addEventListener('change', inspectSource);
document.querySelector('#drive-letter').addEventListener('change', () => {
  updateInstallPathPreview();
  saveState();
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
    await saveState();
    const message = await invoke('launch_omniscience');
    appendOutput(`\n${message}\n`);
  } catch (error) {
    appendOutput(`\nНе удалось запустить: ${String(error)}\n`);
  }
});

const state = await getState();
document.querySelector(`input[name="mode"][value="${state.mode === 'dev' ? 'dev' : 'prod'}"]`).checked = true;
document.querySelector('#source-directory').value = state.sourceDirectory || '';
document.querySelector('#production-executable').value = state.productionExecutable || '';
document.querySelector('#drive-letter').value = state.driveLetter || 'C';
document.querySelector('#github-url').value = state.browserUrl || 'https://github.com/EssenceOfDream/The_Omniscience/releases';
document.querySelector('#launch-summary').textContent =
  state.mode === 'dev'
    ? `Dev · ${state.sourceDirectory || 'каталог не задан'}`
    : `Prod · ${state.productionExecutable || 'EXE не задан'}`;
updateInstallPathPreview();

await inspectSource();
await checkPrerequisites();
await checkLatestRelease();
