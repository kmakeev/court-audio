# Архитектура каркаса «Аудиопротокол»

Документ описывает скелет, заложенный на **этапе 00** (`promts/00_infra.md`), и
точки расширения по этапам 01–08. Зафиксированные решения и граница
ответственности с `ex_system` — в [`../CLAUDE.md`](../CLAUDE.md).

## Состав

- **Ядро захвата** — Rust (`src-tauri/`), библиотека `court_audio_lib`.
- **Оболочка** — Tauri 2.x (системный webview).
- **UI** — React 19 + TS + Vite на дизайн-системе PravoUI
  (вендоренный снапшот — см. [`../.design-sync/SOURCE.md`](../.design-sync/SOURCE.md)).

На этапе 00 рабочей логики записи нет — только собирающийся и запускающийся
каркас: окно с навигацией по четырём экранам, модель `Settings` и её персист.

## Карта Rust-модулей → этапы

Каждый модуль `src-tauri/src/<name>/mod.rs` — заготовка профильного этапа с
doc-комментарием о будущей роли.

| Модуль | Этап | Назначение |
|---|---|---|
| `audio/` | 01 | Устройства, поток `cpal`, кольцевой буфер, метры, ресемпл |
| `recorder/` | 02 | Автомат сессии, сегментный сброс, ротация |
| `reliability/` | 02 | Watchdog, обрыв устройства, контроль диска, восстановление |
| `store/` | 03 | SQLite-манифест, шифрование at-rest, ретеншн |
| `integrity/` | 03 | SHA-256 по сегментам, хеш-цепочка, журнал событий |
| `sync/` | 06 | Возобновляемая чанковая выгрузка в `ex_system`, оффлайн-очередь |
| `player/` | 10.1 | Дешифровка/склейка сегментов на лету, вывод звука, аудит доступа |
| `export/` | 10.2 | Сборка экспортного пакета: побайтовая склейка, FLAC, HTML-плеер, DVD |
| `ipc/` | 00+ | Команды Tauri для UI (сейчас — `get_settings`/`save_settings`) |
| `settings.rs` | 00 | Типизированная схема настроек (реестр `docs/configuration.md`) |

Приём и обработка записи — **в `ex_system`** (этап 07), здесь не дублируется.

## Конфигурация

`settings.rs` зеркалит раздел станции реестра
[`configuration.md`](configuration.md); значения по умолчанию заданы в
`impl Default` (никаких «магических чисел» в логике). UI читает/пишет модель
через IPC-команды `get_settings`/`save_settings`; персист — JSON-файл
`settings.json` в системном config-каталоге приложения
(идентификатор `ru.court.audioprotocol`). TS-зеркало модели —
`src/lib/settings.ts`.

## UI

- Маршрутизация (`react-router-dom`): экраны **Запись · Сессии · Настройки ·
  Диагностика**.
- Оболочка `src/shell/AppShell.tsx` — тёмная шапка + боковая навигация, своя
  (не `Header` из `ex_system`, который app-coupled). Стилизация — только токены
  PravoUI.
- Компоненты PravoUI — в `src/design/` (вендоренный снапшот). Спрайт иконок
  обслуживается по `/icons.svg` (`public/icons.svg`).

## CI

`.github/workflows/ci.yml` — матрица **macOS + Ubuntu + Windows**. Шаги:
`cargo fmt --check`, `cargo clippy -D warnings`, `cargo check`, `npm run build`
(`tsc` + Vite), `npm run tauri build`; артефакты сборки прикладываются под всю
матрицу (Linux `deb`/`rpm`/`appimage`, Windows `nsis`/`msi`, macOS `dmg`).
Сборка под Astra SE / РЕД ОС — ручной workflow
[`package-domestic.yml`](../.github/workflows/package-domestic.yml) на совместимом
билдере (этап 08). Упаковка, оффлайн-установка, подпись и обновления —
[`packaging.md`](packaging.md).

## Открытые вопросы

- **Bundle identifier / правообладатель.** Плейсхолдер `ru.court.audioprotocol`
  сохранён на этапе 08 (решение с пользователем 2026-07-01: не выдумывать).
  Перед подачей в Реестр отечественного ПО заказчик предоставляет финальный
  идентификатор и правообладателя — правки в `src-tauri/tauri.conf.json` и
  [`registry-checklist.md`](registry-checklist.md).
