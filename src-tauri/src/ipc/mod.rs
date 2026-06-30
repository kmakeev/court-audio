//! Команды Tauri для UI (`promts/00_infra.md`, шаг 6).
//!
//! Этап 00 — чтение/сохранение модели [`Settings`]. Этап 01 — команды захвата
//! звука и события уровня/состояния (см. [`audio_cmds`]). Выгрузка и расширенная
//! диагностика — этапы 06+.

use std::fs;
use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::settings::Settings;

pub mod audio_cmds;
pub mod case_cmds;
pub mod query_cmds;

/// Имя файла настроек в каталоге конфигурации приложения.
const SETTINGS_FILE: &str = "settings.json";

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

/// Загрузить настройки из файла конфигурации (или дефолты реестра при его
/// отсутствии). Не-командный помощник: используется командами захвата для сборки
/// параметров записи без дублирования логики.
pub fn load_settings(app: &AppHandle) -> Result<Settings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("не удалось прочитать {}: {e}", path.display()))?;
    serde_json::from_str(&raw)
        .map_err(|e| format!("не удалось разобрать настройки {}: {e}", path.display()))
}

/// Загрузить настройки. При отсутствии файла возвращаются дефолты из реестра
/// (см. [`Settings::default`]); повреждённый файл не валит приложение.
#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<Settings, String> {
    load_settings(&app)
}

/// Сохранить настройки в файл конфигурации (создаёт каталог при отсутствии).
#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let path = settings_path(&app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("не удалось создать каталог {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("не удалось сериализовать настройки: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("не удалось записать {}: {e}", path.display()))
}

/// Корень локального хранилища: `storage.root_path` или `<data-dir>/recordings`.
/// Общий помощник для команд захвата ([`audio_cmds`]) и запросов
/// ([`query_cmds`]) — единый источник пути без дублирования.
pub(crate) fn resolve_storage_root(app: &AppHandle, settings: &Settings) -> Result<PathBuf, String> {
    match &settings.storage.root_path {
        Some(p) => Ok(PathBuf::from(p)),
        None => Ok(app
            .path()
            .app_data_dir()
            .map_err(|e| format!("не удалось определить каталог данных: {e}"))?
            .join("recordings")),
    }
}
