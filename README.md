# Omniscience Updater

Отдельный менеджер установки, запуска и обновления The_Omniscience.

## Задуманное поведение

- **Prod** запускает готовый production-артефакт.
- **Dev** запускает изменяемый checkout проекта через его штатный Tauri dev-сценарий.
- Новые релизы только уведомляются и устанавливаются после явного подтверждения.
- Пользовательские изменения исходников никогда не перезаписываются автоматически.
- В будущем Omniscience сможет запросить production-сборку; активный бинарник заменяется только после успешной сборки и закрытия приложения.

Подробное handoff-ТЗ хранится в основном репозитории в `docs/architecture/omniscience-bootstrapper-and-modes-spec.md`.

## Что сейчас есть

- Панель режима Dev/Prod с сохранением настроек в локальном состоянии приложения.
- Проверка prerequisites через существующий PowerShell-сценарий и показ JSON-результата в UI.
- Проверка последнего stable release через GitHub Releases API.
- Безопасная подготовка конкретного release tag: clone в новый checkout или checkout в уже установленном чистом рабочем дереве.
- Запуск Omniscience в Dev или через настроенный production EXE.
- Встроенное GitHub-окно с адресной строкой и автоматическими кнопками установки `.exe` как Prod и исходного кода как Dev.
- Выбор буквы диска для установки по пути `${диск}:\The_Omniscience\${owner}\${repository}\${tag}\${prod|dev}`.
- `fragmenter/` — побайтовая копия каталога из коммита `fe21ab57e70d3d5ffa89953722236944b5548e2e` основного репозитория.
- `scripts/Restore-Payload.ps1` — восстановление бинарных фрагментов до установки Node.js.
- `scripts/Test-Prerequisites.ps1` — read-only проверка доступности Git, Node.js/npm, Rust/Cargo, Python, MSVC Build Tools и App Installer/winget.
- Tauri + Vite UI с кнопками установки App Installer/winget и Microsoft Store.
- PowerShell-установщики запускаются через `@lydell/node-pty`; окно показывает вывод PTY, но не предоставляет ввод.
- Для production-сборки `scripts/stage-tauri-resources.mjs` упаковывает Node runtime, node-pty и выбранные payload-фрагменты.

Фрагменты — это входные файлы, не готовая установка. В текущем прототипе пакеты x64 устанавливаются в профиль текущего пользователя, уже установленный пакет не обновляется и не понижается. Поддержка Store зависит от версии и политики Windows.

## Проверка среды

Текстовый отчёт:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Test-Prerequisites.ps1
```

Для автоматизации доступен JSON:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Test-Prerequisites.ps1 -Json
```

Проверка сообщает наличие инструментов, но пока не сверяет их с зафиксированными версиями проекта и ничего не устанавливает. Windows App Execution Aliases не запускаются, чтобы диагностика случайно не открыла Microsoft Store.

## Восстановление локальных payload-файлов

Из корня этого репозитория:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Restore-Payload.ps1
```

Файлы будут собраны в `fragmenter/originals/`. Для восстановления только выбранных файлов можно передать `-NamePattern`; сами кнопки используют фиксированные списки. Скрипт проверяет нумерацию частей и пишет через временный файл. Он не сверяет содержимое с отдельными checksum-манифестами.

## Запуск приложения

```powershell
npm ci
npm run tauri:dev
```

Для production-сборки (с включением локальных пакетов-фрагментов):

```powershell
npm run tauri:build
```

Сборка подготовит большой установщик: он включает локальные AppX/MSIX payload-файлы. Установка реальных системных пакетов запускается только после явного нажатия кнопки; этот этап пока не проверяется на целевой Windows и может быть заблокирован политиками или несовместимостью версии ОС.

## Следующие шаги

1. Проверить реальные установки на чистых VM Windows 10/11 x64 и документировать ограничения Store/App Installer.
2. Зафиксировать поддерживаемые версии Windows, Node.js, Rust и MSVC Build Tools.
3. Провести UI-проверку сохранения состояния и подготовки release на реальном рабочем checkout.
4. Реализовать production-сборку с rollback после проверки текущего Tauri build pipeline.

Это первый UI-прототип установочных операций, не готовый updater.
