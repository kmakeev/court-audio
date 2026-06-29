//! Tauri-команды захвата звука (этап 01 — `promts/01_audio_core.md`, шаг 6).
//!
//! Команды: `list_audio_devices`, `start_capture`, `stop_capture`,
//! `pause_capture`, `resume_capture`. События: `audio_level` (пик/RMS) и
//! `capture_state` (idle/recording/paused/stopped). Параметры записи берутся из
//! [`Settings`] (реестр `docs/configuration.md`) — магических чисел нет.
//!
//! Слой IPC — единственное место с Tauri-зависимостью: ядро захвата
//! (`crate::audio`, `crate::recorder`) остаётся Tauri-agnostic и тестируемым.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::audio::capture::{CaptureParams, CaptureSession};
use crate::audio::devices::{list_input_devices, DeviceInfo};
use crate::ipc::load_settings;
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

/// Полезная нагрузка события `capture_state`.
#[derive(Debug, Clone, Serialize)]
struct CaptureStateEvent {
    state: &'static str,
}

const EVENT_CAPTURE_STATE: &str = "capture_state";
const EVENT_AUDIO_LEVEL: &str = "audio_level";

/// Перечислить устройства ввода и их возможности.
#[tauri::command]
pub fn list_audio_devices() -> Result<Vec<DeviceInfo>, String> {
    list_input_devices().map_err(|e| e.to_string())
}

/// Начать захват: подобрать устройство, поднять конвейер и писать сегменты в
/// каталог сессии. Ошибка, если запись уже идёт.
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
    let output_dir = resolve_session_dir(&app, &settings)?;
    let params = build_params(&settings, output_dir.clone());

    let app_for_level = app.clone();
    let level_cb = Box::new(move |level| {
        let _ = app_for_level.emit(EVENT_AUDIO_LEVEL, level);
    });

    let session = CaptureSession::start(params, level_cb).map_err(|e| e.to_string())?;
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

fn emit_state(app: &AppHandle, state: &'static str) {
    let _ = app.emit(EVENT_CAPTURE_STATE, CaptureStateEvent { state });
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

/// Каталог сессии: `<storage.root_path|<data-dir>/recordings>/session-<unix_ms>`.
fn resolve_session_dir(app: &AppHandle, settings: &Settings) -> Result<PathBuf, String> {
    let root = match &settings.storage.root_path {
        Some(p) => PathBuf::from(p),
        None => app
            .path()
            .app_data_dir()
            .map_err(|e| format!("не удалось определить каталог данных: {e}"))?
            .join("recordings"),
    };
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(root.join(format!("session-{unix_ms}")))
}
