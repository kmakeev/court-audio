//! Команды Tauri для UI (`promts/00_infra.md`, шаг 6).
//!
//! Этап 00 — чтение/сохранение модели [`Settings`]. Этап 01 — команды захвата
//! звука и события уровня/состояния (см. [`audio_cmds`]). Выгрузка и расширенная
//! диагностика — этапы 06+.

use std::fs;
use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::settings::Settings;

pub mod admin_cmds;
pub mod audio_cmds;
pub mod auth_cmds;
pub mod case_cmds;
pub mod export_cmds;
pub mod marker_cmds;
pub mod player_cmds;
pub mod query_cmds;
pub mod selftest_cmds;
pub mod settings_gate;
pub mod sync_cmds;
pub mod ui_cmds;

/// Имя файла настроек в каталоге конфигурации приложения.
const SETTINGS_FILE: &str = "settings.json";

/// Сообщение отказа при повреждённом файле настроек (R-003, этап 13.5).
/// Fail-secure: битый `settings.json` **не ослабляет** гейты — операция
/// блокируется с понятной админу диагностикой, а не выполняется «в открытом
/// режиме» на неизвестной политике.
pub const CONFIG_CORRUPT_MESSAGE: &str =
    "конфигурация повреждена — обратитесь к администратору";

/// Имя файла SQLite-манифеста станции в корне хранилища (этап 03).
pub(crate) const MANIFEST_FILE: &str = "manifest.sqlite";

/// Путь к файлу настроек в системном config-каталоге приложения
/// (резолвится Tauri по идентификатору `ru.court.audioprotocol`).
fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("не удалось определить каталог конфигурации: {e}"))?;
    Ok(dir.join(SETTINGS_FILE))
}

/// Разобрать содержимое файла настроек **fail-secure** (R-003, этап 13.5):
/// повреждённый JSON НЕ даёт проницаемую конфигурацию (никаких тихих дефолтов
/// вместо реального файла) — возвращается ошибка с понятным админу сообщением,
/// по которой гейты смыкаются. Чистая функция (тестируется без Tauri).
pub fn parse_settings_failsecure(raw: &str) -> Result<Settings, String> {
    serde_json::from_str::<Settings>(raw)
        .map_err(|e| format!("{CONFIG_CORRUPT_MESSAGE} (детали разбора: {e})"))
}

/// Загрузить настройки из файла конфигурации (или дефолты реестра при его
/// **отсутствии**). Не-командный помощник: используется командами захвата для
/// сборки параметров записи без дублирования логики.
///
/// **Fail-secure (R-003):** отсутствие файла — штатная чистая станция (дефолты
/// реестра, где все `required = true`); **присутствующий, но нечитаемый/битый**
/// файл — не дефолты, а ошибка, чтобы гейты смыкались, а не размыкались на
/// неизвестной политике. Порча дополнительно логируется на старте.
pub fn load_settings(app: &AppHandle) -> Result<Settings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| {
        eprintln!("ВНИМАНИЕ: не удалось прочитать {}: {e}", path.display());
        format!("{CONFIG_CORRUPT_MESSAGE} (детали чтения: {e})")
    })?;
    parse_settings_failsecure(&raw).map_err(|e| {
        eprintln!(
            "ВНИМАНИЕ: {} повреждён — гейты применяются fail-secure ({e})",
            path.display()
        );
        e
    })
}

/// Загрузить настройки. При **отсутствии** файла возвращаются дефолты из реестра
/// (см. [`Settings::default`]); **повреждённый** файл не валит процесс, но
/// возвращает ошибку (fail-secure): UI показывает диагностику, гейты не
/// размыкаются (R-003).
#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<Settings, String> {
    load_settings(&app)
}

/// Записать настройки в файл конфигурации (создаёт каталог при отсутствии).
/// Не-командный помощник: используется командой сохранения и импортом профиля
/// ([`admin_cmds`]) после прохождения гейта разграничения доступа.
pub(crate) fn write_settings(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("не удалось создать каталог {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("не удалось сериализовать настройки: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("не удалось записать {}: {e}", path.display()))
}

/// Env-переключатель провижининга автономного офлайн-режима (B-001).
pub const AUTONOMOUS_OFFLINE_ENV: &str = "COURT_AUDIO_AUTONOMOUS_OFFLINE";

/// Truthy-значения env-флага (регистронезависимо). Чистая функция — юнит-тест.
pub fn env_flag_enabled(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Провизионировать автономный офлайн-режим из env при **отсутствующем**
/// `settings.json` (замечание станции, 2026-07).
///
/// **Зачем.** В изолированном зале включить флаг
/// `auth.operator.autonomous_offline.enabled` через UI **невозможно**: экран
/// «Администрирование» — за гейтом входа оператора (`RequireOperator`), а войти
/// без флага (и без онлайн-связи) нельзя — «курица и яйцо». Прочий провижининг
/// станции уже идёт из env (ключ станции, админ-PIN, профиль оператора), поэтому
/// и флаг режима закрываем тем же путём: при заданном
/// `COURT_AUDIO_AUTONOMOUS_OFFLINE=1` и **отсутствующем** файле настроек ядро
/// один раз пишет дефолтные настройки реестра с включённым флагом. Дальше
/// стандартный провижининг операторского профиля (см. [`crate::run`]) засеет PIN.
///
/// **Идемпотентно и безопасно:** существующий `settings.json` **не трогается**
/// (админские правки/импорт профиля не перезаписываются), поэтому режим нельзя
/// «случайно» включить env на уже настроенной станции — только на чистой.
/// Возвращает `true`, если файл был записан.
pub fn seed_autonomous_offline_from_env_if_absent(app: &AppHandle) -> Result<bool, String> {
    match std::env::var(AUTONOMOUS_OFFLINE_ENV) {
        Ok(raw) if env_flag_enabled(&raw) => {}
        _ => return Ok(false),
    }
    let path = settings_path(app)?;
    if path.exists() {
        // Файл уже есть — уважаем настройку станции, не перезаписываем.
        return Ok(false);
    }
    let mut settings = Settings::default();
    settings.auth.operator.autonomous_offline.enabled = true;
    write_settings(app, &settings)?;
    Ok(true)
}

/// Сохранить настройки (этап 10.4): гейт разграничения оператор/админ на уровне
/// ядра, подтверждение опасных изменений и журнал — в [`admin_cmds`]. UI-запрет
/// недостаточен: оператор не может изменить админ-параметр даже в обход UI.
#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    admin: tauri::State<'_, admin_cmds::AdminState>,
    settings: Settings,
    confirm_dangerous: bool,
) -> Result<admin_cmds::SaveOutcome, String> {
    admin_cmds::apply_settings_save(
        &app,
        &admin,
        settings,
        crate::store::settings_audit::ChangeSource::Manual,
        false,
        confirm_dangerous,
    )
}

/// Ключ станции для чтения/реконсиляции `.enc`-сегментов (R-013, этап 13.7):
/// `Some` при включённом `storage.encrypt_at_rest` и доступном ключе, иначе
/// `None` (plaintext-сессии читаются без ключа). Недоступность ключа здесь не
/// ошибка — громкий fail-secure отказ даёт гейт старта записи и self-test;
/// команды чтения на plaintext-записях продолжают работать.
pub(crate) fn station_key_for_read(
    settings: &Settings,
    storage_root: &std::path::Path,
) -> Option<[u8; 32]> {
    if !settings.storage.encrypt_at_rest {
        return None;
    }
    crate::store::crypto::resolve_station_key(settings.storage.key_source, storage_root).ok()
}

/// Корень локального хранилища: `storage.root_path` или `<data-dir>/recordings`.
/// Общий помощник для команд захвата ([`audio_cmds`]) и запросов
/// ([`query_cmds`]) — единый источник пути без дублирования.
pub(crate) fn resolve_storage_root(
    app: &AppHandle,
    settings: &Settings,
) -> Result<PathBuf, String> {
    match &settings.storage.root_path {
        Some(p) => Ok(PathBuf::from(p)),
        None => Ok(app
            .path()
            .app_data_dir()
            .map_err(|e| format!("не удалось определить каталог данных: {e}"))?
            .join("recordings")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Провижининг автономного режима из env (замечание станции 2026-07): чистая
    // логика распознавания truthy-флага. Сама запись settings.json требует
    // AppHandle и проверяется на станции по чек-листу docs/first_run_offline.md.
    #[test]
    fn autonomous_offline_env_flag_truthy_values() {
        for v in ["1", "true", "TRUE", "Yes", " on ", "On"] {
            assert!(env_flag_enabled(v), "ожидали truthy для {v:?}");
        }
        for v in ["", "0", "false", "no", "off", "enabled?", "2"] {
            assert!(!env_flag_enabled(v), "ожидали falsy для {v:?}");
        }
    }
}
