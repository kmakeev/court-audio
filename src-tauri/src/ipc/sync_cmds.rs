//! Tauri-команды и фоновый цикл агента выгрузки (этап 06 —
//! `promts/06_sync_agent.md`, шаг 6).
//!
//! Тонкий слой над движком [`crate::sync`] (как `case_cmds` над стором): команды
//! резолвят пути/настройки и меняют состояние выгрузки в манифесте; саму сетевую
//! работу ведёт фоновый планировщик [`spawn_scheduler`] — он **не на горячем
//! пути захвата** и уважает приоритет записи. UI получает один тип ошибки (`String`).
//!
//! Операторские действия: `retry_upload` (сбросить ошибку → в очередь),
//! `pause_upload`/`resume_upload`. Прогресс/статусы UI читает через
//! [`super::query_cmds::list_sessions`] (поля прогресса из `upload_parts`).

use std::path::PathBuf;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::ipc::audio_cmds::CaptureState;
use crate::ipc::{load_settings, resolve_storage_root, MANIFEST_FILE};
use crate::reliability::watchdog::now_unix_ms;
use crate::store::crypto;
use crate::store::manifest::{ManifestStore, SessionStatus, UploadStatus};
use crate::store::reconcile;
use crate::sync::client::HttpTransport;
use crate::sync::scheduler::{process_queue_once, TickContext};
use crate::sync::{queue, OPERATOR_TOKEN_ENV};

/// Открыть манифест станции по настройкам приложения.
fn open_store(app: &AppHandle) -> Result<(ManifestStore, PathBuf), String> {
    let settings = load_settings(app)?;
    let root = resolve_storage_root(app, &settings)?;
    let store = ManifestStore::open(&root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;
    Ok((store, root))
}

/// Реконсилировать сессию из каталога и вернуть её `id` (как `bind_session_case`).
fn session_id_for_dir(store: &ManifestStore, dir: &str) -> Result<String, String> {
    reconcile::reconcile_session(store, &PathBuf::from(dir))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("каталог не содержит начатой сессии: {dir}"))
}

/// Повторить выгрузку записи: сбросить статус ошибки в `pending`, очистить
/// ошибки неотправленных частей и снять паузу — фоновый планировщик подхватит.
#[tauri::command]
pub fn retry_upload(app: AppHandle, dir: String) -> Result<(), String> {
    let (store, _) = open_store(&app)?;
    let id = session_id_for_dir(&store, &dir)?;
    let session = store
        .get_session(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("сессия {id} не найдена"))?;
    // Подтверждённую/удалённую запись не «повторяем».
    if session.upload_status == UploadStatus::Confirmed || session.status == SessionStatus::Purged {
        return Err("запись уже подтверждена или удалена — повтор не нужен".to_string());
    }
    store
        .set_upload_status(&id, UploadStatus::Pending)
        .map_err(|e| e.to_string())?;
    store
        .set_upload_paused(&id, false)
        .map_err(|e| e.to_string())?;
    queue::reset_for_retry(&store, &id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Поставить выгрузку записи на паузу (планировщик её пропускает).
#[tauri::command]
pub fn pause_upload(app: AppHandle, dir: String) -> Result<(), String> {
    let (store, _) = open_store(&app)?;
    let id = session_id_for_dir(&store, &dir)?;
    store
        .set_upload_paused(&id, true)
        .map_err(|e| e.to_string())
}

/// Снять паузу выгрузки записи.
#[tauri::command]
pub fn resume_upload(app: AppHandle, dir: String) -> Result<(), String> {
    let (store, _) = open_store(&app)?;
    let id = session_id_for_dir(&store, &dir)?;
    store
        .set_upload_paused(&id, false)
        .map_err(|e| e.to_string())
}

/// Операторский токен выгрузки. До этапа `auth` — из env [`OPERATOR_TOKEN_ENV`]
/// (секрет не в settings.json). Нет токена → планировщик копит очередь, не теряя.
fn operator_token() -> Option<String> {
    std::env::var(OPERATOR_TOKEN_ENV)
        .ok()
        .filter(|t| !t.is_empty())
}

/// Запустить фоновый планировщик выгрузки. Низкоприоритетный поток: периодически
/// (idle-кадр = `sync.retry.backoff_max_ms`, из реестра — без нового параметра)
/// прогоняет очередь, если задан `sync.server_base_url`. Не мешает захвату:
/// отдельный поток вне аудио-пути + флаг `defer_during_recording`.
pub fn spawn_scheduler(app: AppHandle) {
    std::thread::Builder::new()
        .name("court-audio-sync".to_string())
        .spawn(move || scheduler_loop(app))
        .expect("не удалось запустить поток планировщика выгрузки");
}

fn scheduler_loop(app: AppHandle) {
    loop {
        let idle = match run_pass(&app) {
            Ok(idle) => idle,
            // Сбой настройки/манифеста не должен ронять поток — пробуем позже.
            Err(_) => Duration::from_millis(default_idle_ms()),
        };
        std::thread::sleep(idle);
    }
}

/// Дефолтный idle-интервал, если настройки не прочитались (реестр: backoff_max).
fn default_idle_ms() -> u64 {
    crate::settings::SyncSettings::default()
        .retry
        .backoff_max_ms as u64
}

/// Один проход планировщика; возвращает интервал до следующего опроса.
fn run_pass(app: &AppHandle) -> Result<Duration, String> {
    let settings = load_settings(app)?;
    let idle = Duration::from_millis(settings.sync.retry.backoff_max_ms as u64);

    // Нет адреса сервера — выгружать некуда, просто ждём (idle).
    let Some(base_url) = settings.sync.server_base_url.clone() else {
        return Ok(idle);
    };
    let root = resolve_storage_root(app, &settings)?;
    let store = ManifestStore::open(&root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;

    let transport = match HttpTransport::new(base_url) {
        Ok(t) => t,
        Err(e) => return Err(e.to_string()),
    };

    // Ключ станции для дешифрования сегментов (если шифрование включено).
    let key = if settings.storage.encrypt_at_rest {
        crypto::resolve_station_key(settings.storage.key_source, &root).ok()
    } else {
        None
    };

    let is_recording = app
        .try_state::<CaptureState>()
        .map(|s| s.0.lock().map(|g| g.is_some()).unwrap_or(false))
        .unwrap_or(false);

    let token = operator_token();
    let ctx = TickContext {
        token: token.as_deref(),
        is_recording,
        now_unix_ms: now_unix_ms(),
        settings: &settings,
        key: key.as_ref(),
    };
    // Результаты прохода не пробрасываем в UI напрямую — статус виден через
    // манифест (list_sessions); ошибки отдельных записей уже накоплены внутри.
    let _ = process_queue_once(&store, &transport, &ctx);
    Ok(idle)
}
