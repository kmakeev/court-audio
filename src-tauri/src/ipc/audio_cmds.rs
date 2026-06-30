//! Tauri-команды захвата звука (этап 01 — `promts/01_audio_core.md`, шаг 6) +
//! события/команды надёжности (этап 02 — `promts/02_recorder_reliability.md`).
//!
//! Команды: `list_audio_devices`, `start_capture`, `stop_capture`,
//! `pause_capture`, `resume_capture`, `scan_recoverable`, `recover_session`,
//! `discard_session`. События: `audio_level` (пик/RMS), `capture_state`
//! (idle/recording/paused/stopping/stopped) и `reliability_warning`
//! (диск/watchdog/устройство/длина сессии). Параметры записи берутся из
//! [`Settings`] (реестр `docs/configuration.md`) — магических чисел нет.
//!
//! Слой IPC — единственное место с Tauri-зависимостью: ядро захвата
//! (`crate::audio`, `crate::recorder`, `crate::reliability`) остаётся
//! Tauri-agnostic и тестируемым.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::audio::capture::{
    CaptureParams, CaptureSession, DiskWatch, ReliabilityConfig, ReliabilityEvent,
};
use crate::audio::devices::{list_input_devices, DeviceInfo};
use crate::ipc::load_settings;
use crate::recorder::journal::{Journal, JournalRecord};
use crate::recorder::recovery;
use crate::reliability::disk_monitor::DiskThresholds;
use crate::settings::Settings;

/// Управляемое Tauri состояние активной сессии захвата.
#[derive(Default)]
pub struct CaptureState(pub Mutex<Option<CaptureSession>>);

/// Ответ `start_capture`: фактический формат и каталог сессии.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureStarted {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub output_dir: String,
}

/// Краткое описание записанного сегмента (заглушка манифеста до этапа 03).
#[derive(Debug, Clone, Serialize)]
pub struct SegmentSummary {
    pub index: u32,
    pub path: String,
    pub frames: u64,
    pub started_at_unix_ms: String,
}

/// Незавершённая сессия для UI восстановления.
#[derive(Debug, Clone, Serialize)]
pub struct RecoverableSession {
    pub dir: String,
    pub completed_segments: u32,
    pub already_recovered: bool,
}

/// Полезная нагрузка события `capture_state`.
#[derive(Debug, Clone, Serialize)]
struct CaptureStateEvent {
    state: &'static str,
}

const EVENT_CAPTURE_STATE: &str = "capture_state";
const EVENT_AUDIO_LEVEL: &str = "audio_level";
const EVENT_RELIABILITY_WARNING: &str = "reliability_warning";

/// Перечислить устройства ввода и их возможности.
#[tauri::command]
pub fn list_audio_devices() -> Result<Vec<DeviceInfo>, String> {
    list_input_devices().map_err(|e| e.to_string())
}

/// Начать захват: подобрать устройство, поднять конвейер (с журналом и
/// надёжностью) и писать сегменты в каталог сессии. Ошибка, если запись уже идёт.
#[tauri::command]
pub fn start_capture(
    app: AppHandle,
    state: State<'_, CaptureState>,
) -> Result<CaptureStarted, String> {
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "состояние захвата повреждено".to_string())?;
    if guard.is_some() {
        return Err("запись уже идёт".to_string());
    }

    let settings = load_settings(&app)?;
    let storage_root = resolve_storage_root(&app, &settings)?;
    let output_dir = session_dir(&storage_root);
    let params = build_params(&settings, output_dir.clone());

    // Журнал сессии (write-ahead): открываем до старта, сразу пишем SessionStarted.
    let journal = Journal::open(&output_dir).map_err(|e| e.to_string())?;
    let journal = Arc::new(Mutex::new(journal));
    {
        let started_at_unix_ms = now_unix_ms();
        let mut g = journal.lock().map_err(|_| "журнал повреждён".to_string())?;
        g.append(&JournalRecord::SessionStarted {
            started_at_unix_ms,
            sample_rate_hz: settings.audio.sample_rate_hz,
            channels: settings.audio.channels,
            bit_depth: settings.audio.bit_depth,
            segment_seconds: settings.recorder.segment_seconds,
        })
        .map_err(|e| e.to_string())?;
    }

    let app_for_level = app.clone();
    let level_cb = Box::new(move |level| {
        let _ = app_for_level.emit(EVENT_AUDIO_LEVEL, level);
    });

    let app_for_warn = app.clone();
    let on_event: crate::audio::capture::ReliabilityCallback =
        Arc::new(move |ev: ReliabilityEvent| {
            let _ = app_for_warn.emit(EVENT_RELIABILITY_WARNING, ev);
        });

    let reliability = build_reliability(&settings, &storage_root, Arc::clone(&journal), on_event);

    let session =
        CaptureSession::start(params, level_cb, reliability).map_err(|e| e.to_string())?;
    let native = session.native_format();
    *guard = Some(session);
    drop(guard);

    emit_state(&app, "recording");
    Ok(CaptureStarted {
        sample_rate_hz: native.sample_rate_hz,
        channels: native.channels,
        output_dir: output_dir.to_string_lossy().into_owned(),
    })
}

/// Остановить захват, финализировать сегменты и вернуть их список.
#[tauri::command]
pub fn stop_capture(
    app: AppHandle,
    state: State<'_, CaptureState>,
) -> Result<Vec<SegmentSummary>, String> {
    let session = {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "состояние захвата повреждено".to_string())?;
        guard
            .take()
            .ok_or_else(|| "запись не запущена".to_string())?
    };

    emit_state(&app, "stopping");
    let segments = session.stop().map_err(|e| e.to_string())?;
    emit_state(&app, "stopped");
    Ok(segments
        .into_iter()
        .map(|s| SegmentSummary {
            index: s.index,
            path: s.path.to_string_lossy().into_owned(),
            frames: s.frames,
            // u128 не сериализуется serde_json как число — отдаём строкой.
            started_at_unix_ms: s.started_at_unix_ms.to_string(),
        })
        .collect())
}

/// Поставить активный захват на паузу.
#[tauri::command]
pub fn pause_capture(app: AppHandle, state: State<'_, CaptureState>) -> Result<(), String> {
    {
        let guard = state
            .0
            .lock()
            .map_err(|_| "состояние захвата повреждено".to_string())?;
        let session = guard
            .as_ref()
            .ok_or_else(|| "запись не запущена".to_string())?;
        session.pause().map_err(|e| e.to_string())?;
    }
    emit_state(&app, "paused");
    Ok(())
}

/// Возобновить захват после паузы.
#[tauri::command]
pub fn resume_capture(app: AppHandle, state: State<'_, CaptureState>) -> Result<(), String> {
    {
        let guard = state
            .0
            .lock()
            .map_err(|_| "состояние захвата повреждено".to_string())?;
        let session = guard
            .as_ref()
            .ok_or_else(|| "запись не запущена".to_string())?;
        session.resume().map_err(|e| e.to_string())?;
    }
    emit_state(&app, "recording");
    Ok(())
}

/// Найти незавершённые сессии (для UI восстановления при старте приложения).
#[tauri::command]
pub fn scan_recoverable(app: AppHandle) -> Result<Vec<RecoverableSession>, String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;
    let found = recovery::scan_unfinished(&root).map_err(|e| e.to_string())?;
    Ok(found
        .into_iter()
        .map(|s| RecoverableSession {
            dir: s.dir.to_string_lossy().into_owned(),
            completed_segments: s.completed_segments.len() as u32,
            already_recovered: s.recovered,
        })
        .collect())
}

/// Восстановить сессию «на месте»: починить последний сегмент и пометить
/// сессию восстановленной (решение заказчика — дописываем ту же сессию).
#[tauri::command]
pub fn recover_session(dir: String) -> Result<(), String> {
    recovery::recover_in_place(&PathBuf::from(dir))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Закрыть незавершённую сессию как восстановленную, не продолжая запись:
/// чиним последний сегмент (целостность данных) и помечаем `Recovered`+`Stopped`.
#[tauri::command]
pub fn discard_session(dir: String) -> Result<(), String> {
    let dir = PathBuf::from(dir);
    recovery::recover_in_place(&dir).map_err(|e| e.to_string())?;
    let mut j = Journal::open(&dir).map_err(|e| e.to_string())?;
    j.append(&JournalRecord::Stopped).map_err(|e| e.to_string())
}

fn emit_state(app: &AppHandle, state: &'static str) {
    let _ = app.emit(EVENT_CAPTURE_STATE, CaptureStateEvent { state });
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Собрать параметры захвата из настроек (реестр — единственный источник).
fn build_params(settings: &Settings, output_dir: PathBuf) -> CaptureParams {
    CaptureParams {
        device: settings.audio.device.clone(),
        desired_sample_rate_hz: settings.audio.sample_rate_hz,
        target_channels: settings.audio.channels,
        bit_depth: settings.audio.bit_depth,
        capture_buffer_seconds: settings.audio.capture_buffer_seconds,
        level_update_hz: settings.audio.level_update_hz,
        segment_seconds: settings.recorder.segment_seconds,
        flush_interval_ms: settings.recorder.flush_interval_ms,
        output_dir,
    }
}

/// Собрать конфигурацию надёжности из настроек (все пороги/таймауты — из реестра).
fn build_reliability(
    settings: &Settings,
    storage_root: &std::path::Path,
    journal: Arc<Mutex<Journal>>,
    on_event: crate::audio::capture::ReliabilityCallback,
) -> ReliabilityConfig {
    let r = &settings.reliability;
    let mirror_dir = if r.mirror.enabled {
        r.mirror.path.as_ref().map(PathBuf::from)
    } else {
        None
    };
    ReliabilityConfig {
        journal: Some(journal),
        mirror_dir,
        disk: Some(DiskWatch {
            // Свободное место меряем по тому корня хранилища.
            path: storage_root.to_path_buf(),
            thresholds: DiskThresholds {
                low_mb: r.disk_low_threshold_mb,
                critical_mb: r.disk_critical_mb,
            },
        }),
        max_session: Some(std::time::Duration::from_secs(
            settings.recorder.max_session_hours as u64 * 3600,
        )),
        watchdog_timeout: Some(std::time::Duration::from_millis(
            r.watchdog_timeout_ms as u64,
        )),
        auto_resume: r.device_reconnect.auto_resume,
        device_name: settings.audio.device.clone(),
        on_event: Some(on_event),
    }
}

/// Корень локального хранилища: `storage.root_path` или `<data-dir>/recordings`.
fn resolve_storage_root(app: &AppHandle, settings: &Settings) -> Result<PathBuf, String> {
    match &settings.storage.root_path {
        Some(p) => Ok(PathBuf::from(p)),
        None => Ok(app
            .path()
            .app_data_dir()
            .map_err(|e| format!("не удалось определить каталог данных: {e}"))?
            .join("recordings")),
    }
}

/// Каталог конкретной сессии: `<storage_root>/session-<unix_ms>`.
fn session_dir(storage_root: &std::path::Path) -> PathBuf {
    storage_root.join(format!("session-{}", now_unix_ms()))
}
