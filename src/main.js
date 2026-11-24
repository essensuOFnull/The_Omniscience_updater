import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './styles.css';

const log = document.querySelector('#log');
const status = document.querySelector('#status');
const hint = document.querySelector('#log-hint');
const buttons = [...document.querySelectorAll('button[data-action]')];
const ansiEscape = /\u001b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])/g;
let active = false;

function setBusy(isBusy, label = 'Готово') {
  active = isBusy;
  for (const button of buttons) button.disabled = isBusy;
  status.textContent = isBusy ? 'Установка выполняется…' : label;
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

await listen('installer-output', (event) => {
  appendOutput(event.payload.text);
});

await listen('installer-finished', (event) => {
  const { success, exitCode } = event.payload;
  setBusy(false, success ? 'Установка завершена' : `Ошибка (код ${exitCode})`);
  hint.textContent = success ? 'Процесс PowerShell завершён' : 'Проверьте журнал и ограничения Windows';
  appendOutput(success
    ? '\n\nОперация завершена.\n'
    : `\n\nОперация завершилась с ошибкой (код ${exitCode}).\n`);
});

for (const button of buttons) {
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
