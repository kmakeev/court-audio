# Упаковка и дистрибутивы «Аудиопротокол» (этап 08)

Документ описывает сборку устанавливаемых дистрибутивов v1 под целевую матрицу
ОС, **оффлайн-установку** в закрытом контуре суда, аудит рантайм-зависимостей,
подпись пакетов и **контролируемые обновления без публичного авто-апдейта**.

Источник истины по метаданным упаковки —
[`../src-tauri/tauri.conf.json`](../src-tauri/tauri.conf.json) (блок `bundle`).
Рантайм-настройки станции — [`configuration.md`](configuration.md); этап 08
новых рантайм-параметров не вводит.

> **Границы:** ГОСТ ЭЦП бинаря и юридически значимая подпись записи — фаза 2
> (`promts/11_gost_signing.md`). ALT Linux — фаза 2. Фактическая подача в Реестр
> отечественного ПО — организационный шаг вне кода (чек-лист —
> [`registry-checklist.md`](registry-checklist.md)).

## Настройки vs метаданные упаковки

Принцип «никаких магических чисел» относится к **рантайм-логике** станции: её
параметры живут в [`configuration.md`](configuration.md) и модели `Settings`.
**Метаданные упаковки** — имена/версии системных зависимостей, фиксированная
версия WebView2, publisher/copyright, форматы таргетов — это *build*-данные;
их законное место — `tauri.conf.json` и workflow'ы CI, а не реестр настроек.
Смешивать эти области не нужно.

**Машинно-зависимые настройки при тиражировании (R-005).** Профиль станции
(`Settings`) переносим между залами, **кроме** привязанных к конкретной машине/ОС
полей: `audio.device` (имя устройства уникально для ОС), `storage.root_path` и
`reliability.mirror.path` (абсолютные пути). При переносе конфига **очищайте** их
(`audio.device = null`, пустые пути → системные каталоги). Иначе перенесённый с
другой машины `audio.device` даёт «молчаливую тишину» захвата (экран «Запись» и
self-test предупреждают явно, но чистка при переносе — правильный путь), а
несуществующие пути ломают запись/зеркало. Реестр и таблица полей —
[`configuration.md`](configuration.md) («Машинно-зависимые настройки»).

## Матрица форматов

| ОС | Формат | Таргет Tauri | Назначение |
|---|---|---|---|
| Astra Linux SE | `.deb` | `deb` | Целевой деплой (Debian-based) |
| РЕД ОС | `.rpm` | `rpm` | Целевой деплой (RPM-based) |
| Windows 10/11 | MSI / NSIS + portable | `nsis` (`msi`) | Целевой деплой |
| macOS | `.dmg` | `dmg` | **Только разработка**, не деплой |

Таргеты объявлены явным списком в `bundle.targets`:
`["deb", "rpm", "appimage", "nsis", "dmg"]`. AppImage — вспомогательный
переносимый Linux-вариант; ALT и иные ОС — фаза 2.

**Portable под Windows:** NSIS-инсталлятор (`installMode: both`) ставит
per-user или per-machine без прав администратора в per-user режиме. Полностью
«portable» запуск (распаковка без установки) достигается извлечением содержимого
NSIS или отдельной раздачей каталога сборки `target/release/` вместе с
оффлайн-рантаймом WebView2 (см. ниже) — процедура для изолированных станций.

## Оффлайн-установка (закрытый контур)

Залы суда бывают изолированы от интернета. Ни один дистрибутив не должен тянуть
компоненты из сети во время установки.

### Windows — бандл фиксированного WebView2 Runtime

`tauri.conf.json` → `bundle.windows.webviewInstallMode`:

```json
{ "type": "fixedRuntime", "path": "webview2/" }
```

Это **бандл фиксированной версии** WebView2 Runtime внутрь инсталлятора (не
онлайн-бутстраппер). Рантайм кладётся в
[`../src-tauri/webview2/`](../src-tauri/webview2/README.md) перед сборкой; сами
бинарники — вне git (лицензия Microsoft, размер), в CI подкладываются секретом
`WEBVIEW2_FIXED_URL`. Итоговый MSI/NSIS ставится на чистой Windows 10/11 без
доступа в сеть.

> В CI без секрета `WEBVIEW2_FIXED_URL` собирается обычный установщик
> (`downloadBootstrapper`) — он **не** оффлайн; оффлайн-артефакт выпускается на
> релизном стенде/с секретом. Оба пути — в
> [`.github/workflows/ci.yml`](../.github/workflows/ci.yml).

### Astra Linux SE / РЕД ОС — системные зависимости из локального репозитория

Linux-пакет объявляет рантайм-зависимости (`webkit2gtk`, аудио, TLS) в
метаданных (`bundle.linux.deb.depends` / `bundle.linux.rpm.depends`). В закрытом
контуре зависимости ставятся из **локального (зеркального) репозитория ОС** или
заранее скачанного набора пакетов:

**Astra SE (.deb):**
```bash
# Из локального репозитория/каталога с .deb:
sudo apt-get install --no-download ./audioprotocol_0.1.0_amd64.deb
# apt подтянет объявленные depends из локального репозитория:
#   libwebkit2gtk-4.1-0 libgtk-3-0 libayatana-appindicator3-1
#   librsvg2-2 libasound2 libssl3
```

**РЕД ОС (.rpm):**
```bash
# dnf/yum из локального репозитория:
sudo dnf install ./audioprotocol-0.1.0-1.x86_64.rpm
# depends: webkit2gtk4.1 gtk3 libayatana-appindicator-gtk3
#          librsvg2 alsa-lib openssl-libs
```

Если локального репозитория нет — заранее собрать набор зависимостей
(`apt-get download` / `dnf download --resolve`) на машине той же ОС/версии и
перенести на станцию.

## Аудит рантайм-зависимостей по ОС

Полный список того, что должно присутствовать на станции **во время работы**.

| Компонент | Astra SE (deb) | РЕД ОС (rpm) | Windows | Роль |
|---|---|---|---|---|
| Webview | `libwebkit2gtk-4.1-0` | `webkit2gtk4.1` | WebView2 Runtime (бандл) | Оболочка UI |
| GTK | `libgtk-3-0` | `gtk3` | — | Тулкит webkit |
| Индикатор трея | `libayatana-appindicator3-1` | `libayatana-appindicator-gtk3` | — | Системный индикатор |
| SVG | `librsvg2-2` | `librsvg2` | — | Иконки |
| Аудио | `libasound2` (ALSA); PulseAudio при наличии | `alsa-lib` | WASAPI (в ОС) | Захват звука (`cpal`) |
| TLS | `libssl3` | `openssl-libs` | Schannel (в ОС) | Выгрузка (см. прим.) |

**Примечания импортозамещения:**
- Захват — через `cpal` (ALSA/PulseAudio на Linux, WASAPI на Windows,
  CoreAudio на macOS); ОС-специфики хардкодом в коде нет.
- Сетевой стек выгрузки использует **rustls** (`reqwest` без OpenSSL —
  `default-features=false`, `rustls-tls`), поэтому системный OpenSSL для работы
  модуля выгрузки **не требуется**; `libssl3`/`openssl-libs` в depends — для
  webkit-зависимостей ОС, не для нашего TLS.
- SQLite — `rusqlite` c `bundled` (свой libsqlite внутри бинаря), системный
  пакет не нужен.

## Доступ к микрофону по ОС

Модель выдачи доступа к микрофону различается; захват идёт через `cpal`, но
разрешение — забота ОС/бандла. Сводка и что нужно для каждой ОС:

| ОС | Модель | Что требуется | Симптом при отказе |
|---|---|---|---|
| **macOS** (dev) | TCC, per-app запрос | `NSMicrophoneUsageDescription` в `Info.plist`; для подписанного/hardened — entitlement `com.apple.security.device.audio-input` | Запрос не показан → тишина |
| **Linux** (Astra/РЕД ОС/Ubuntu) | Прямой доступ к устройству (ALSA/PulseAudio/PipeWire) | Ничего: нативный `.deb`/`.rpm` не в песочнице | — (доступ есть по умолчанию) |
| **Windows** 10/11 | Системная приватность, без per-app запроса для desktop-приложений | Включён тумблер «Разрешить классическим приложениям доступ к микрофону» | Тишина (WASAPI отдаёт нули) |

**macOS.** Оба ключа — в бандле: [`../src-tauri/Info.plist`](../src-tauri/Info.plist)
(мержится Tauri) и [`../src-tauri/entitlements.plist`](../src-tauri/entitlements.plist)
(подключён `bundle.macOS.entitlements`). При первом старте записи ОС показывает
запрос доступа. Если ранее стоял старый бандл без ключа — сбросить кэш решения:
`tccutil reset Microphone ru.court.audioprotocol`.

**Linux.** Per-app модели разрешений у нативных пакетов нет — отдельный запрос
не появляется и не нужен. (Проблема возможна лишь при упаковке во Flatpak/Snap
с песочницей — мы их не используем; при переходе — портал `Device`/`Microphone`.)

**Windows.** Классические установщики (NSIS/MSI) — не MSIX/UWP, поэтому
manifest-capability и per-app запрос не применяются. Но есть **системный**
тумблер приватности (Параметры → Конфиденциальность → Микрофон → «Разрешить
классическим приложениям доступ к микрофону»); если он выключен — симптом такой
же, как был на macOS (тишина). Это **эксплуатационная** настройка ОС, правкой
упаковки не решается — включается администратором станции. Внести в руководство
администратора.

## Подпись пакетов

Подпись обязательна для контролируемой поставки. Ключи — только в
секретах/контуре, **не в репозитории**.

### Windows (code signing)

`bundle.windows.digestAlgorithm = sha256`. Отпечаток сертификата и таймстамп
подставляются при сборке через переменные окружения/секреты CI (например,
`certificateThumbprint`, `timestampUrl`), а не хардкодом в `tauri.conf.json`.
При наличии HSM/токена — через `signCommand` (кастомная команда подписи).

### Linux (GPG)

`.deb`/`.rpm` и локальный репозиторий подписываются GPG-ключом контура:
- `.rpm` — `rpm --addsign` ключом, публичная часть импортируется в ОС
  (`rpm --import`);
- `.deb`/репозиторий — подпись `Release`/`InRelease` (`gpg`/`reprepro`), ключ
  добавляется в доверенные (`/etc/apt/trusted.gpg.d/`).

Ключи хранятся в секретах CI (`secrets.GPG_PRIVATE_KEY` и т. п.) или на
защищённом стенде подписи. ГОСТ-подпись бинаря (КриптоПро) — фаза 2 (`11`).

### macOS (Developer ID + нотаризация)

Релизный `release.yml` собирает macOS-бандл (`.dmg`, `aarch64`) **без подписи и
нотаризации** — секретов подписи в контуре пока нет. Следствие: скачанный из
GitHub `.app` несёт карантинный атрибут `com.apple.quarantine`, и Gatekeeper
(особенно на Apple Silicon) отказывает запуск с сообщением **«приложение
повреждено, переместите в Корзину»**. Приложение при этом целое — блокирует
именно проверка подписи/карантина **до** запуска (к файлам конфигурации
`~/Library/Application Support/ru.court.audioprotocol/` отношения не имеет).

**Запуск неподписанной сборки (обходной приём для проверочного контура).** Снять
карантин и поставить ad-hoc подпись (последнее нужно на Apple Silicon):

```bash
APP="/Applications/Аудиопротокол.app"   # или путь, куда установлен .app
xattr -dr com.apple.quarantine "$APP"   # снять карантин
codesign --force --deep -s - "$APP"     # ad-hoc подпись (Apple Silicon)
open "$APP"
```

**Боевая поставка — подпись Developer ID + нотаризация.** Чтобы `.dmg`
открывался штатным двойным кликом без `xattr`, в `release.yml` (шаг сборки
macOS через `tauri-action`) прокинуть секреты Apple: сертификат **Developer ID
Application** и учётку нотаризации — `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`,
`APPLE_PASSWORD` (app-specific), `APPLE_TEAM_ID`. `tauri-action` подписывает и
нотаризует из коробки при их наличии. Ключи — только в секретах CI/контуре, не
в репозитории. Отечественные ОС (Astra SE / РЕД ОС) macOS-подписи не требуют —
там подпись `.deb`/`.rpm` GPG (см. выше).

## Обновления — контролируемые, без авто-апдейта

**Решение (2026-07-01): ручные подписанные пакеты.** Публичного авто-апдейта
нет; tauri-updater-плагин **не подключён** (в `tauri.conf.json` нет
`plugins.updater`, в зависимостях нет `tauri-plugin-updater`). Обновление =
установка нового **подписанного** `.deb`/`.rpm`/MSI администратором станции.

**Будущая опция (отложено): внутренний update-канал.** При наличии
инфраструктуры суда можно поднять внутренний (локальный) сервер обновлений и
подключить tauri-updater c `endpoints` на этот сервер (без интернета) +
подпись апдейтов ключом обновлений. Это добавит рантайм-настройку адреса
канала — вводить её через [`configuration.md`](configuration.md), не хардкодом.
До согласования инфраструктуры — не реализуется.

## Версионирование и идентификаторы

- **Семантическая версия** синхронизирована в
  [`../package.json`](../package.json), [`../src-tauri/Cargo.toml`](../src-tauri/Cargo.toml)
  и `tauri.conf.json` (сейчас `0.1.0`). Журнал изменений —
  [`../CHANGELOG.md`](../CHANGELOG.md).
- **Идентификатор станции** — env-seam `COURT_AUDIO_STATION_ID`
  (`src-tauri/src/sync/mod.rs`); попадает в метаданные выгрузки и журнал.
- **Bundle identifier / правообладатель** — плейсхолдер `ru.court.audioprotocol`
  и обобщённый `publisher`/`copyright`. **Финальные значения согласовать с
  заказчиком** до подачи в Реестр (см. [`registry-checklist.md`](registry-checklist.md)).

## Провижининг секретов станции (обязательно при развёртывании)

Секреты станции **не хранятся** в `settings.json` — задаются через переменные
окружения на зале при развёртывании. Три env-seam'а, все читаются ядром:

| Env | Назначение | Обязательность |
|---|---|---|
| `COURT_AUDIO_STATION_PASSPHRASE` | Парольная фраза → **ключ станции** (Argon2id): шифрует `admin_pin.enc`, `auth_session.enc`, `case_cache.enc` и (при `storage.encrypt_at_rest`) аудио-сегменты | **Обязательна** (R-004): без неё офлайн-вход по PIN, админ-PIN и шифрование ПДн не работают |
| `COURT_AUDIO_ADMIN_PIN` | Админ-PIN → Argon2id-хеш в `admin_pin.enc` на первом запуске (дальше env можно снять) | Обязателен, если админ-настройки правятся на станции (`admin.pin.required = true`) |
| `COURT_AUDIO_STATION_ID` | Идентификатор станции (учётка транспорта выгрузки) | Обязателен для выгрузки |

**Ключ станции обязателен и валидируется на старте (R-004, этап 13.5).** Раньше
отсутствие фразы деградировало **молча**: онлайн-вход «успешен», а зашифрованные
блобы (кэш офлайн-сессии, админ-PIN, кэш дел, сегменты) не писались — в следующий
раз офлайн-старт по PIN оказывался недоступен без предупреждения. Теперь:

- на старте приложение **проверяет** доступность ключа станции и при отсутствии
  громко диагностирует (лог + пункт self-test «Ключ станции»);
- при включённом `storage.encrypt_at_rest` (дефолт) недоступный ключ — **Fail**
  self-test (запись не сохранится); иначе — **Warn** (офлайн-контур недоступен);
- вход оператора при невозможности сохранить кэш офлайн-сессии **завершается
  явной ошибкой**, а не «тихим успехом».

Порядок (systemd/скрипт запуска зала): экспортировать `COURT_AUDIO_STATION_PASSPHRASE`
(и на первом запуске `COURT_AUDIO_ADMIN_PIN`) **до** старта приложения. Соль KDF
персистится в `<storage.root_path>/key.salt` — фраза должна быть **стабильной**
между рестартами, иначе ключ не воспроизведётся и блобы перестанут читаться.
Реальная интеграция OS-keystore (keychain/secret-service/DPAPI) как альтернатива
env-фразе — перспектива этого же этапа `08` (сейчас `os_keystore` — заглушка с
фолбэком на фразу). См. также реестр настроек — [`configuration.md`](configuration.md).

## Сборка

### Локальная сборка по ОС

Ниже — воспроизводимые шаги сборки на чистой машине каждой ОС. Кросс-сборка
между ОС Tauri не выполняет: `.deb`/`.rpm`/AppImage собираются на Linux,
MSI/NSIS — на Windows, `.dmg` — на macOS. Общие предпосылки везде: **Rust
stable** (rustup), **Node.js 20+**, исходники репозитория.

Артефакты после сборки — в `src-tauri/target/release/bundle/<формат>/`.

#### 1. Ubuntu (Linux, первый целевой шаг перед Astra SE / РЕД ОС)

Ubuntu — базовый Linux-таргет: тот же тулчейн и системные зависимости, что и на
Astra (Debian-based). Сначала проверяем сборку здесь, затем переносим процедуру
на отечественные ОС.

```bash
# 1) Системные зависимости Tauri + аудио + сборка rpm-таргета
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev \
  libayatana-appindicator3-dev libssl-dev libasound2-dev \
  patchelf rpm            # rpm/rpmbuild — если нужен и .rpm

# 2) Toolchain (если ещё нет)
curl https://sh.rustup.rs -sSf | sh -s -- -y     # Rust stable
# Node.js 20+ — из nvm/nodesource

# 3) Сборка
npm ci
npm run tauri build                     # deb + rpm + appimage (по bundle.targets)
# или точечно:
npm run tauri build -- --bundles deb    # только .deb (как на Astra SE)
```

Выход: `bundle/deb/*.deb`, `bundle/rpm/*.rpm`, `bundle/appimage/*.AppImage`.

#### 2. Windows 10/11

```powershell
# 1) Предпосылки:
#    - Visual Studio Build Tools (MSVC) + Windows SDK (C/C++ toolchain для Rust)
#    - Rust stable (rustup, target x86_64-pc-windows-msvc)
#    - Node.js 20+
#    - NSIS (для .exe-инсталлятора) — Tauri подтягивает при сборке
#
# 2) Оффлайн-рантайм WebView2 (fixedRuntime): положить фикс-версию рантайма в
#    src-tauri\webview2\  (см. src-tauri\webview2\README.md). Без него сборка
#    Windows-инсталлятора не пройдёт — либо распаковать рантайм, либо временно
#    переопределить режим (см. ниже).

npm ci
npm run tauri build                      # nsis (.exe) + msi, с оффлайн-WebView2

# Быстрая сборка БЕЗ оффлайн-рантайма (обычный установщик, тянет WebView2 из сети —
# только для локальной проверки, НЕ для контура суда):
npm run tauri build -- --config '{\"bundle\":{\"windows\":{\"webviewInstallMode\":{\"type\":\"downloadBootstrapper\"}}}}'
```

Выход: `bundle\nsis\*.exe`, `bundle\msi\*.msi`. Подпись — см. «Подпись пакетов».

#### 3. macOS (только разработка)

```bash
npm install
npm run tauri build                      # .dmg (dev, не целевой деплой)
# точечно:
npm run tauri build -- --bundles dmg
```

Выход: `bundle/dmg/*.dmg`. macOS-бандл содержит `Info.plist`
(`NSMicrophoneUsageDescription`) и `entitlements.plist` — см. «Доступ к
микрофону по ОС».

### CI (macOS + Ubuntu + Windows)

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) собирает под матрицу и
прикладывает артефакты: Linux (`deb`/`rpm`/`appimage`), Windows (`nsis`/`msi`),
macOS (`dmg`, dev). Для `.rpm` на ubuntu-раннере ставится пакет `rpm`. Артефакты
прогона — временные (страница run → *Artifacts* или `gh run download <id>`), не
Release.

### Релизы (GitHub Release по тегу)

[`.github/workflows/release.yml`](../.github/workflows/release.yml) на пуш тега
`vX.Y.Z` собирает матрицу и публикует **черновик релиза** (draft) с
установщиками; публикацию подтверждает человек вручную.

```bash
git tag v0.1.0
git push origin v0.1.0     # → сборка → draft Release с артефактами
```

`GITHUB_TOKEN` выдаётся Actions автоматически — доп. секреты для самого релиза не
нужны. Секреты (Repository secrets) требуются только для боевого качества:
`WEBVIEW2_FIXED_URL` (оффлайн-Windows), Windows code signing, `GPG_*` (Linux) —
без них draft собирается неподписанным, а Windows — в онлайн-варианте
(`downloadBootstrapper`). Как только секрет `WEBVIEW2_FIXED_URL` задан, тот же
workflow собирает оффлайн-Windows без правок. Отечественные `.deb`/`.rpm` —
стенд ([`package-domestic.yml`](../.github/workflows/package-domestic.yml)).

### Отечественные ОС (Astra SE / РЕД ОС)

GitHub-hosted раннеров под эти ОС нет. Сборка — на **совместимом билдере**:
self-hosted раннер (или контейнер с образом ОС) с меткой `astra-se` / `red-os`.
Ручной запуск —
[`.github/workflows/package-domestic.yml`](../.github/workflows/package-domestic.yml)
(`workflow_dispatch`, выбор целевой ОС). Процедура на стенде:

1. Поднять ОС нужной версии (Astra SE / РЕД ОС), поставить toolchain: Rust
   stable, Node 20, системные dev-зависимости (webkit/gtk/alsa/ssl, `rpm-build`
   для РЕД ОС) из локального репозитория.
2. `npm ci && npm run build`.
3. `npm run tauri build -- --bundles deb` (Astra) или `--bundles rpm` (РЕД ОС).
4. Подписать пакет GPG-ключом контура, разложить в локальный репозиторий.
5. Проверить установку на **чистой** ОС без интернета (см. «Оффлайн-установка»).

## Критерии приёмки этапа (напоминание)

- Дистрибутив ставится на чистых Astra SE / РЕД ОС / Windows 10/11 **без
  интернета**, приложение запускается и ведёт запись.
- Windows-установка не тянет рантайм из сети (WebView2 в бандле).
- Пакеты подписаны; обновление — подписанным пакетом (без авто-апдейта).
- CI выпускает артефакты под матрицу; процедура под отеч. ОС задокументирована
  (этот файл).
- Чек-лист Реестра заполнен — [`registry-checklist.md`](registry-checklist.md).
