//! Команды Tauri для UI (`promts/00_infra.md`, шаг 6).
//!
//! На этапе 00 — только чтение/сохранение модели [`Settings`]. Команды записи
//! звука, выгрузки и диагностики добавятся на этапах 01+.

use std::fs;
use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::settings::Settings;

/// Имя файла настроек в каталоге конфигурации приложения.
const SETTINGS_FILE: &str = "settings.json";

/// Путь к файлу настроек в системном config-каталоге приложения
/// (резолвится Tauri по идентификатору `ru.court.audioprotocol`).
fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("не удалось определить каталог конфигурации: {e}"))?;
    Ok(dir.join(SETTINGS_FILE))
}

/// Загрузить настройки. При отсутствии файла возвращаются дефолты из реестра
/// (см. [`Settings::default`]); повреждённый файл не валит приложение.
#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<Settings, String> {
    let path = settings_path(&app)?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("не удалось прочитать {}: {e}", path.display()))?;
    serde_json::from_str(&raw)
        .map_err(|e| format!("не удалось разобрать настройки {}: {e}", path.display()))
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
