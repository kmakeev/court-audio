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

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::audio::capture::{
    CaptureParams, CaptureSession, DiskWatch, LevelEvent, MultiMonitor, ReliabilityConfig,
    ReliabilityEvent,
};
use crate::audio::devices::{list_input_devices, DeviceInfo};
use crate::audio::tracks::{resolve_tracks, ResolvedTrack};
use crate::ipc::{load_settings, resolve_storage_root};
use crate::ipc::marker_cmds::AnnotationState;
use crate::recorder::journal::{Journal, JournalRecord};
use crate::recorder::multitrack::{
    self, track_dir, track_map_from_resolved, MultiCapture, TrackMap, TrackStartSpec,
};
use crate::recorder::recovery;
use crate::reliability::disk_monitor::DiskThresholds;
use crate::settings::Settings;

/// Активный захват: одноканальный (v1) или многоканальный (этап 09). Обе ветки
/// управляются единообразно (пауза/резюм/стоп), разнятся лишь способом старта.
pub enum ActiveCapture {
    Single(CaptureSession),
    Multi(MultiCapture),
}

impl ActiveCapture {
    fn pause(&self) -> Result<(), crate::audio::AudioError> {
        match self {
            ActiveCapture::Single(s) => s.pause(),
            ActiveCapture::Multi(m) => m.pause(),
        }
    }
    fn resume(&self) -> Result<(), crate::audio::AudioError> {
        match self {
            ActiveCapture::Single(s) => s.resume(),
            ActiveCapture::Multi(m) => m.resume(),
        }
    }
    fn is_paused(&self) -> bool {
        match self {
            ActiveCapture::Single(s) => s.is_paused(),
            ActiveCapture::Multi(m) => m.is_paused(),
        }
    }
}

/// Активная сессия захвата + её метаданные для восстановления статуса в UI
/// (этап 04): без них UI после перехода между вкладками не знал бы, что запись
/// продолжается в фоне.
pub struct ActiveSession {
    pub capture: ActiveCapture,
    pub started_at_unix_ms: u64,
    pub output_dir: PathBuf,
    /// Подкаталоги дорожек для журнала `Stopped` на стопе (многоканал).
    pub track_subdirs: Vec<PathBuf>,
    /// Частота оси сессии (Гц) — для семпл-точных смещений разметки (этап 10).
    pub sample_rate_hz: u32,
    /// Автор разметки (оператор) — до экрана входа берётся из env (этап 10).
    pub operator_id: String,
    /// Карта дорожек (многоканал) — подстановка роли активной дорожки (этап 10).
    pub track_map: Option<TrackMap>,
    /// Живая разметка сессии (этап 10): write-ahead журнал + in-memory лог.
    pub annotations: Mutex<AnnotationState>,
}

/// Управляемое Tauri состояние активной сессии захвата.
#[derive(Default)]
pub struct CaptureState(pub Mutex<Option<ActiveSession>>);

/// Управляемое Tauri состояние активного мониторинга уровня (без записи).
/// Многоканал (этап 13.2): [`MultiMonitor`] держит по превью-потоку на дорожку
/// (одноканал — один поток, `track_id 0`).
#[derive(Default)]
pub struct MonitorState(pub Mutex<Option<MultiMonitor>>);

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
    /// Дорожка сегмента (многоканал — этап 09; для v1 — `0`).
    pub track_id: u32,
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
    monitor: State<'_, MonitorState>,
) -> Result<CaptureStarted, String> {
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "состояние захвата повреждено".to_string())?;
    if guard.is_some() {
        return Err("запись уже идёт".to_string());
    }

    // Освобождаем устройство от мониторинга: запись и монитор не делят поток.
    stop_monitor_internal(&monitor)?;

    let settings = load_settings(&app)?;
    // Гейт входа (этап 10.3): при `auth.operator.required_to_start` старт новой
    // сессии требует вошедшего оператора. Идущую запись это не касается.
    crate::ipc::auth_cmds::ensure_start_allowed(&app, &settings)?;
    let storage_root = resolve_storage_root(&app, &settings)?;
    // Fail-secure гейт шифрования (R-013, этап 13.7): при включённом
    // `storage.encrypt_at_rest` ключ станции обязателен ДО старта — никакого
    // тихого plaintext-фолбэка (линия R-003/R-004). Self-test «Ключ станции»
    // показывает тот же отказ заранее.
    let segment_key = segment_encryption_key(&settings, &storage_root)?;
    let output_dir = session_dir(&storage_root);
    let started_at_unix_ms = now_unix_ms();

    // Многоканал (этап 09): включён и задана карта дорожек → N потоков по ролям.
    // Иначе — одноканальный путь v1 (поведение без изменений).
    let tracks = resolve_tracks(&settings).map_err(|e| e.to_string())?;
    let multichannel = settings.audio.multichannel.enabled && !settings.audio.tracks.is_empty();

    let started = if multichannel {
        start_multichannel(
            &app,
            &settings,
            &storage_root,
            &output_dir,
            started_at_unix_ms,
            &tracks,
            segment_key,
        )?
    } else {
        start_single(
            &app,
            &settings,
            &storage_root,
            &output_dir,
            started_at_unix_ms,
            segment_key,
        )?
    };

    *guard = Some(started.active);
    drop(guard);

    emit_state(&app, "recording");
    Ok(started.reply)
}

/// Внутренний результат старта: активная сессия + ответ UI.
struct Started {
    active: ActiveSession,
    reply: CaptureStarted,
}

/// Одноканальный старт (v1): один поток, downmix к `audio.channels`.
fn start_single(
    app: &AppHandle,
    settings: &Settings,
    storage_root: &std::path::Path,
    output_dir: &std::path::Path,
    started_at_unix_ms: u64,
    segment_key: Option<[u8; 32]>,
) -> Result<Started, String> {
    let params = build_params(settings, output_dir.to_path_buf());

    // Идентичность (этап 10.3): оператор — из сессии входа, станция — учётка
    // станции. Пишем в журнал (write-ahead), чтобы доехала до манифеста/выгрузки.
    let operator_id = operator_identity(app);
    let station_id = crate::ipc::auth_cmds::station_identity();
    // B-001 (этап 13.6): сессия открыта автономным офлайн-стартом по PIN?
    let autonomous_offline = crate::ipc::auth_cmds::current_operator_autonomous(app);

    // Журнал сессии (write-ahead): открываем до старта, сразу пишем SessionStarted.
    let journal = Journal::open(output_dir).map_err(|e| e.to_string())?;
    let journal = Arc::new(Mutex::new(journal));
    {
        let mut g = journal.lock().map_err(|_| "журнал повреждён".to_string())?;
        g.append(&JournalRecord::SessionStarted {
            started_at_unix_ms,
            sample_rate_hz: settings.audio.sample_rate_hz,
            channels: settings.audio.channels,
            bit_depth: settings.audio.bit_depth,
            segment_seconds: settings.recorder.segment_seconds,
            operator_id: operator_id.clone(),
            station_id,
            autonomous_offline,
        })
        .map_err(|e| e.to_string())?;
    }

    // Зеркало метаданных при старте (этап 13.3): корневой журнал с `SessionStarted`
    // едет на второй носитель сразу — зеркало реконсилируемо даже при сбое до стопа.
    mirror_session_metadata(
        settings,
        storage_root,
        &[output_dir.join(crate::recorder::journal::JOURNAL_FILE_NAME)],
    );

    let app_for_level = app.clone();
    let level_cb = Box::new(move |level| {
        let _ = app_for_level.emit(EVENT_AUDIO_LEVEL, level);
    });

    let reliability = build_reliability(
        settings,
        storage_root,
        Arc::clone(&journal),
        warn_cb(app),
        settings.audio.device.clone(),
        segment_key,
    );

    let session =
        CaptureSession::start(params, level_cb, reliability).map_err(|e| e.to_string())?;
    let native = session.native_format();
    Ok(Started {
        active: ActiveSession {
            capture: ActiveCapture::Single(session),
            started_at_unix_ms,
            output_dir: output_dir.to_path_buf(),
            track_subdirs: Vec::new(),
            // Ось разметки — фактическая (нативная) частота записи (этап 10).
            sample_rate_hz: native.sample_rate_hz,
            operator_id,
            track_map: None,
            // Тот же журнал, что у reliability: записи разметки и сегментов в один
            // файл сериализуются общим мьютексом (этап 10).
            annotations: Mutex::new(AnnotationState::new(journal)),
        },
        reply: CaptureStarted {
            sample_rate_hz: native.sample_rate_hz,
            channels: native.channels,
            output_dir: output_dir.to_string_lossy().into_owned(),
        },
    })
}

/// Многоканальный старт (этап 09): по одному потоку `CaptureSession` на дорожку в
/// подкаталог `track-<id>-<role>/`; общий таймкод; карта дорожек — в `tracks.json`.
fn start_multichannel(
    app: &AppHandle,
    settings: &Settings,
    storage_root: &std::path::Path,
    output_dir: &std::path::Path,
    started_at_unix_ms: u64,
    tracks: &[ResolvedTrack],
    segment_key: Option<[u8; 32]>,
) -> Result<Started, String> {
    // Идентичность (этап 10.3): оператор из сессии входа, станция — учётка станции.
    let operator_id = operator_identity(app);
    let station_id = crate::ipc::auth_cmds::station_identity();
    let autonomous_offline = crate::ipc::auth_cmds::current_operator_autonomous(app);

    // Карта дорожек (для реконсиляции и UI) + корневой журнал сессии.
    let map = track_map_from_resolved(tracks);
    multitrack::write_track_map(output_dir, &map).map_err(|e| e.to_string())?;
    {
        let mut root = Journal::open(output_dir).map_err(|e| e.to_string())?;
        root.append(&JournalRecord::SessionStarted {
            started_at_unix_ms,
            sample_rate_hz: settings.audio.sample_rate_hz,
            channels: tracks.len() as u16,
            bit_depth: settings.audio.bit_depth,
            segment_seconds: settings.recorder.segment_seconds,
            operator_id: operator_id.clone(),
            station_id: station_id.clone(),
            autonomous_offline,
        })
        .map_err(|e| e.to_string())?;
    }

    // Зеркало метаданных при старте (этап 13.3): `tracks.json` + корневой журнал
    // едут сразу — зеркало опознаётся как многоканальное и реконсилируемо даже
    // при сбое до стопа (per-track журналы/сегменты consumer досылает «на лету»).
    mirror_session_metadata(
        settings,
        storage_root,
        &[
            output_dir.join(multitrack::TRACKS_FILE_NAME),
            output_dir.join(crate::recorder::journal::JOURNAL_FILE_NAME),
        ],
    );

    let mut specs = Vec::with_capacity(map.tracks.len());
    let mut subdirs = Vec::with_capacity(map.tracks.len());
    for entry in &map.tracks {
        let subdir = track_dir(output_dir, entry);
        subdirs.push(subdir.clone());
        // Per-track журнал (write-ahead) в подкаталоге дорожки.
        let mut tj = Journal::open(&subdir).map_err(|e| e.to_string())?;
        tj.append(&JournalRecord::SessionStarted {
            started_at_unix_ms,
            sample_rate_hz: settings.audio.sample_rate_hz,
            channels: 1,
            bit_depth: settings.audio.bit_depth,
            segment_seconds: settings.recorder.segment_seconds,
            operator_id: operator_id.clone(),
            station_id: station_id.clone(),
            autonomous_offline,
        })
        .map_err(|e| e.to_string())?;
        let tj = Arc::new(Mutex::new(tj));

        let mut params = build_params(settings, subdir.clone());
        params.device = entry.device.clone();
        params.channel_index = Some(entry.channel_index);
        params.track_id = entry.track_id;
        params.target_channels = 1; // дорожка моно

        let reliability = build_reliability(
            settings,
            storage_root,
            tj,
            warn_cb(app),
            entry.device.clone(),
            segment_key,
        );
        specs.push(TrackStartSpec {
            track_id: entry.track_id,
            params,
            reliability,
        });
    }

    let app_for_level = app.clone();
    let level_emit: crate::recorder::multitrack::LevelEmit =
        Arc::new(move |level: LevelEvent| {
            let _ = app_for_level.emit(EVENT_AUDIO_LEVEL, level);
        });

    let multi = MultiCapture::start(specs, level_emit).map_err(|e| e.to_string())?;
    let track_count = multi.track_count() as u16;
    // Журнал разметки — на корне сессии (дорожки пишут в свои подкаталоги, так что
    // конфликта записей нет). Открываем в append (SessionStarted уже записан выше).
    let annotation_journal = Journal::open(output_dir).map_err(|e| e.to_string())?;
    Ok(Started {
        active: ActiveSession {
            capture: ActiveCapture::Multi(multi),
            started_at_unix_ms,
            output_dir: output_dir.to_path_buf(),
            track_subdirs: subdirs,
            // Ось разметки — общая номинальная частота сессии (этап 10).
            sample_rate_hz: settings.audio.sample_rate_hz,
            operator_id,
            track_map: Some(map),
            annotations: Mutex::new(AnnotationState::new(Arc::new(Mutex::new(
                annotation_journal,
            )))),
        },
        reply: CaptureStarted {
            sample_rate_hz: settings.audio.sample_rate_hz,
            channels: track_count,
            output_dir: output_dir.to_string_lossy().into_owned(),
        },
    })
}

/// Колбэк событий надёжности: эмитит `reliability_warning` в UI.
fn warn_cb(app: &AppHandle) -> crate::audio::capture::ReliabilityCallback {
    let app_for_warn = app.clone();
    Arc::new(move |ev: ReliabilityEvent| {
        let _ = app_for_warn.emit(EVENT_RELIABILITY_WARNING, ev);
    })
}

/// Остановить захват, финализировать сегменты и вернуть их список.
#[tauri::command]
pub fn stop_capture(
    app: AppHandle,
    state: State<'_, CaptureState>,
) -> Result<Vec<SegmentSummary>, String> {
    let active = {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "состояние захвата повреждено".to_string())?;
        guard
            .take()
            .ok_or_else(|| "запись не запущена".to_string())?
    };

    emit_state(&app, "stopping");
    // Собираем сегменты обеих веток в единый список (с track_id).
    let mut summaries: Vec<SegmentSummary> = Vec::new();
    match active.capture {
        ActiveCapture::Single(session) => {
            for s in session.stop().map_err(|e| e.to_string())? {
                summaries.push(segment_summary(0, s));
            }
        }
        ActiveCapture::Multi(multi) => {
            for (track_id, segs) in multi.stop().map_err(|e| e.to_string())? {
                for s in segs {
                    summaries.push(segment_summary(track_id, s));
                }
            }
            // Штатное завершение per-track журналов (для реконсиляции статуса).
            for subdir in &active.track_subdirs {
                if let Ok(mut j) = Journal::open(subdir) {
                    let _ = j.append(&JournalRecord::Stopped);
                }
            }
        }
    }
    // Фиксируем штатное завершение в корневом журнале (write-ahead): без этой
    // записи реконсиляция считает сессию незавершённой (статус «recording»).
    {
        let mut j = Journal::open(&active.output_dir).map_err(|e| e.to_string())?;
        j.append(&JournalRecord::Stopped)
            .map_err(|e| e.to_string())?;
    }

    // Финальное зеркалирование метаданных (этап 13.3): корневой журнал (уже с
    // `Stopped`), `tracks.json` и per-track журналы — чтобы по зеркалу сессия
    // реконсилировалась как штатно завершённая. Best-effort; сбой не роняет стоп.
    if let Ok(settings) = load_settings(&app) {
        if let Ok(storage_root) = resolve_storage_root(&app, &settings) {
            let mut files = vec![active
                .output_dir
                .join(crate::recorder::journal::JOURNAL_FILE_NAME)];
            let tracks_json = active.output_dir.join(multitrack::TRACKS_FILE_NAME);
            if tracks_json.exists() {
                files.push(tracks_json);
            }
            for subdir in &active.track_subdirs {
                files.push(subdir.join(crate::recorder::journal::JOURNAL_FILE_NAME));
            }
            mirror_session_metadata(&settings, &storage_root, &files);
        }
    }

    emit_state(&app, "stopped");
    Ok(summaries)
}

/// Свести `SegmentInfo` в UI-сводку с дорожкой.
fn segment_summary(track_id: u32, s: crate::recorder::segment_writer::SegmentInfo) -> SegmentSummary {
    SegmentSummary {
        track_id,
        index: s.index,
        path: s.path.to_string_lossy().into_owned(),
        frames: s.frames,
        // u128 не сериализуется serde_json как число — отдаём строкой.
        started_at_unix_ms: s.started_at_unix_ms.to_string(),
    }
}

/// Поставить активный захват на паузу.
#[tauri::command]
pub fn pause_capture(app: AppHandle, state: State<'_, CaptureState>) -> Result<(), String> {
    {
        let guard = state
            .0
            .lock()
            .map_err(|_| "состояние захвата повреждено".to_string())?;
        let active = guard
            .as_ref()
            .ok_or_else(|| "запись не запущена".to_string())?;
        active.capture.pause().map_err(|e| e.to_string())?;
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
        let active = guard
            .as_ref()
            .ok_or_else(|| "запись не запущена".to_string())?;
        active.capture.resume().map_err(|e| e.to_string())?;
    }
    emit_state(&app, "recording");
    Ok(())
}

/// Запустить мониторинг уровня (без записи): открыть устройство и слать события
/// `audio_level`, чтобы оператор видел работу микрофона до старта записи.
/// Идемпотентно: повторный вызов перезапускает монитор. Если идёт запись — она
/// сама эмитит уровни, отдельный монитор не нужен (ошибка).
#[tauri::command]
pub fn start_monitor(
    app: AppHandle,
    state: State<'_, CaptureState>,
    monitor: State<'_, MonitorState>,
) -> Result<(), String> {
    {
        let cap = state
            .0
            .lock()
            .map_err(|_| "состояние захвата повреждено".to_string())?;
        if cap.is_some() {
            return Err("идёт запись — мониторинг не нужен".to_string());
        }
    }

    // Перезапуск: гасим прежний монитор перед открытием устройства заново.
    stop_monitor_internal(&monitor)?;

    let settings = load_settings(&app)?;
    let storage_root = resolve_storage_root(&app, &settings)?;
    // Состав превью-потоков — та же карта дорожек, что у записи (этап 13.2): в
    // многоканале по потоку на дорожку/канал, иначе один поток v1 (track_id 0).
    let tracks = resolve_tracks(&settings).map_err(|e| e.to_string())?;
    let params = build_monitor_params(&settings, &storage_root, &tracks);

    let app_for_level = app.clone();
    let level_emit: crate::audio::capture::MonitorLevelEmit = Arc::new(move |level: LevelEvent| {
        let _ = app_for_level.emit(EVENT_AUDIO_LEVEL, level);
    });

    let session = MultiMonitor::start(params, level_emit).map_err(|e| e.to_string())?;
    let mut guard = monitor
        .0
        .lock()
        .map_err(|_| "состояние мониторинга повреждено".to_string())?;
    *guard = Some(session);
    Ok(())
}

/// Собрать параметры превью-потоков из карты дорожек (реестр — единственный
/// источник). Многоканал (`audio.multichannel.enabled` + непустой `audio.tracks`):
/// по потоку на дорожку — извлечение своего канала (`channel_index`) под её
/// `track_id`, дорожка моно; совпадает со стартом записи ([`start_multichannel`]).
/// Иначе — один поток v1 (`build_params`: downmix, `track_id 0`), поведение без
/// изменений. `output_dir` монитору не нужен (он не пишет), но `CaptureParams`
/// его требует — передаём корень хранилища.
fn build_monitor_params(
    settings: &Settings,
    storage_root: &std::path::Path,
    tracks: &[ResolvedTrack],
) -> Vec<CaptureParams> {
    let multichannel = settings.audio.multichannel.enabled && !settings.audio.tracks.is_empty();
    if !multichannel {
        return vec![build_params(settings, storage_root.to_path_buf())];
    }
    tracks
        .iter()
        .map(|t| {
            let mut p = build_params(settings, storage_root.to_path_buf());
            p.device = t.device.clone();
            p.channel_index = Some(t.channel_index);
            p.track_id = t.track_id;
            p.target_channels = 1; // дорожка моно
            p
        })
        .collect()
}

/// Остановить мониторинг уровня и освободить устройство.
#[tauri::command]
pub fn stop_monitor(monitor: State<'_, MonitorState>) -> Result<(), String> {
    stop_monitor_internal(&monitor)
}

/// Снять активный монитор (если есть) и корректно остановить его поток.
fn stop_monitor_internal(monitor: &State<'_, MonitorState>) -> Result<(), String> {
    let session = {
        let mut guard = monitor
            .0
            .lock()
            .map_err(|_| "состояние мониторинга повреждено".to_string())?;
        guard.take()
    };
    if let Some(session) = session {
        session.stop();
    }
    Ok(())
}

/// Текущее состояние захвата — чтобы UI восстанавливал статус после перехода
/// между вкладками (запись продолжается в фоне; компонент экрана размонтируется
/// и теряет локальное состояние). Этап 04.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureStatus {
    /// `idle` / `recording` / `paused`.
    pub state: &'static str,
    pub started_at_unix_ms: Option<u64>,
    pub output_dir: Option<String>,
    /// Сколько сегментов уже записано на диск (живой счётчик прогресса).
    pub segment_count: u32,
}

/// Вернуть текущее состояние активной сессии (или `idle`).
#[tauri::command]
pub fn capture_status(state: State<'_, CaptureState>) -> Result<CaptureStatus, String> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "состояние захвата повреждено".to_string())?;
    match guard.as_ref() {
        None => Ok(CaptureStatus {
            state: "idle",
            started_at_unix_ms: None,
            output_dir: None,
            segment_count: 0,
        }),
        Some(active) => Ok(CaptureStatus {
            state: if active.capture.is_paused() {
                "paused"
            } else {
                "recording"
            },
            started_at_unix_ms: Some(active.started_at_unix_ms),
            output_dir: Some(active.output_dir.to_string_lossy().into_owned()),
            segment_count: count_segments(&active.output_dir),
        }),
    }
}

/// Подсчитать записанные сегменты в каталоге сессии (`seg-NNNN-….wav` и — при
/// шифровании at-rest, R-013 — `….wav.enc`). Многоканал (этап 09): сегменты
/// живут в подкаталогах дорожек — считаем и их (один уровень вложенности
/// `track-*/`).
fn count_segments(dir: &std::path::Path) -> u32 {
    fn count_flat(dir: &std::path::Path) -> u32 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|e| {
                e.file_name().to_str().is_some_and(|n| {
                    n.starts_with("seg-") && (n.ends_with(".wav") || n.ends_with(".wav.enc"))
                })
            })
            .count() as u32
    }
    let mut total = count_flat(dir);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.path().is_dir() {
                total += count_flat(&e.path());
            }
        }
    }
    total
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

/// Восстановить сессию «на месте»: починить последний сегмент, дожурналить
/// незажурналированный хвост, дофинализировать его в `.enc` при включённом
/// шифровании (R-013) и пометить сессию восстановленной (решение заказчика —
/// дописываем ту же сессию).
#[tauri::command]
pub fn recover_session(app: AppHandle, dir: String) -> Result<(), String> {
    let key = recovery_encryption_key(&app)?;
    recovery::recover_in_place(&PathBuf::from(dir), key.as_ref())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Закрыть незавершённую сессию как восстановленную, не продолжая запись:
/// чиним последний сегмент (целостность данных) и помечаем `Recovered`+`Stopped`.
#[tauri::command]
pub fn discard_session(app: AppHandle, dir: String) -> Result<(), String> {
    let key = recovery_encryption_key(&app)?;
    let dir = PathBuf::from(dir);
    recovery::recover_in_place(&dir, key.as_ref()).map_err(|e| e.to_string())?;
    let mut j = Journal::open(&dir).map_err(|e| e.to_string())?;
    j.append(&JournalRecord::Stopped).map_err(|e| e.to_string())
}

/// Ключ дофинализации хвостового сегмента для команд восстановления: та же
/// fail-secure политика, что у старта записи (`segment_encryption_key`).
fn recovery_encryption_key(app: &AppHandle) -> Result<Option<[u8; 32]>, String> {
    let settings = load_settings(app)?;
    let root = resolve_storage_root(app, &settings)?;
    segment_encryption_key(&settings, &root)
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

/// Автор разметки/сессий/аудита: с этапа 10.3 — `operator_id` вошедшего оператора
/// ([`crate::ipc::auth_cmds::current_operator_id`]). В тестах/CI (без входа) —
/// фолбэк на env-подпорку `OPERATOR_ID_ENV`. `pub(crate)` — переиспользуется
/// плеером/экспортом.
pub(crate) fn operator_identity(app: &AppHandle) -> String {
    let id = crate::ipc::auth_cmds::current_operator_id(app);
    if !id.is_empty() {
        return id;
    }
    operator_identity_test_fallback()
}

/// Тестовая/CI-подпорка идентичности из env (боевой путь её не использует, если
/// оператор вошёл). Отдельная функция — чтобы `#[cfg(test)]`-фолбэк был явным.
fn operator_identity_test_fallback() -> String {
    std::env::var(crate::sync::OPERATOR_ID_ENV)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_default()
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
        // Одноканальный путь IPC: downmix к target_channels, дорожка 0.
        channel_index: None,
        track_id: 0,
    }
}

/// Собрать конфигурацию надёжности из настроек (все пороги/таймауты — из реестра).
fn build_reliability(
    settings: &Settings,
    storage_root: &std::path::Path,
    journal: Arc<Mutex<Journal>>,
    on_event: crate::audio::capture::ReliabilityCallback,
    device_name: Option<String>,
    segment_key: Option<[u8; 32]>,
) -> ReliabilityConfig {
    let r = &settings.reliability;
    // Зеркало (этап 13.3): структура зеркала = структуре основного места, поэтому
    // передаём корень хранилища как базу для реконструкции дерева сессии/дорожки.
    let mirror = if r.mirror.enabled {
        r.mirror
            .path
            .as_ref()
            .map(|p| crate::audio::capture::MirrorSpec {
                storage_root: storage_root.to_path_buf(),
                mirror_root: PathBuf::from(p),
            })
    } else {
        None
    };
    ReliabilityConfig {
        journal: Some(journal),
        mirror,
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
        device_name,
        on_event: Some(on_event),
        segment_key,
    }
}

/// Разрешить ключ шифрования сегментов по политике `storage.encrypt_at_rest`
/// (R-013). Fail-secure: включённое шифрование без доступного ключа — громкая
/// ошибка (старт записи/восстановление блокируются), не тихий plaintext.
/// `pub` — чистая функция без Tauri, гейт проверяется интеграционным тестом
/// (`tests/gates_failsecure.rs`), как `start_gate_decision`/`admin_change_denied`.
pub fn segment_encryption_key(
    settings: &Settings,
    storage_root: &std::path::Path,
) -> Result<Option<[u8; 32]>, String> {
    if !settings.storage.encrypt_at_rest {
        return Ok(None);
    }
    crate::store::crypto::resolve_station_key(settings.storage.key_source, storage_root)
        .map(Some)
        .map_err(|e| {
            format!(
                "шифрование записей включено (storage.encrypt_at_rest), но ключ станции недоступен: {e}. \
                 Запись невозможна — задайте ключ станции при развёртывании (см. docs/packaging.md) \
                 или отключите шифрование через администратора."
            )
        })
}

/// Каталог конкретной сессии: `<storage_root>/session-<unix_ms>`.
fn session_dir(storage_root: &std::path::Path) -> PathBuf {
    storage_root.join(format!("session-{}", now_unix_ms()))
}

/// Best-effort зеркалирование метаданных сессии (журнал(ы), `tracks.json`) на
/// второй носитель (этап 13.3). Сегменты зеркалирует consumer «на лету»; здесь —
/// метаданные (при старте — чтобы зеркало было реконсилируемо и после сбоя; при
/// стопе — финальное состояние с `Stopped`). Структура зеркала = структуре
/// основного места; сбой только логируется и **не влияет** на запись.
fn mirror_session_metadata(settings: &Settings, storage_root: &Path, files: &[PathBuf]) {
    let r = &settings.reliability;
    if !r.mirror.enabled {
        return;
    }
    let Some(mirror_root) = r.mirror.path.as_ref() else {
        return;
    };
    let mirror = match crate::reliability::mirror::Mirror::new(storage_root, Path::new(mirror_root))
    {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[reliability] зеркало метаданных недоступно ({mirror_root}): {e}");
            return;
        }
    };
    for f in files {
        if f.exists() {
            if let Err(e) = mirror.mirror_file(f) {
                eprintln!("[reliability] зеркалирование метаданных {f:?} не удалось: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::TrackConfig;

    fn track(device: Option<&str>, channel: u16, role: &str) -> TrackConfig {
        TrackConfig {
            device: device.map(|s| s.to_string()),
            channel_index: channel,
            role: role.to_string(),
            label: String::new(),
        }
    }

    #[test]
    fn monitor_params_single_is_one_stream_track0() {
        // Одноканал (v1): один превью-поток, downmix (channel_index None), tid 0.
        let settings = Settings::default();
        let tracks = resolve_tracks(&settings).unwrap();
        let params = build_monitor_params(&settings, std::path::Path::new("/tmp/root"), &tracks);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].track_id, 0);
        assert_eq!(params[0].channel_index, None);
    }

    #[test]
    fn monitor_params_multichannel_covers_all_tracks_including_duplicate_device() {
        // Два канала одного устройства + «дубль» первого канала на третьей дорожке:
        // превью должно поднять поток на каждую дорожку под своим track_id.
        let mut settings = Settings::default();
        settings.audio.multichannel.enabled = true;
        settings.audio.device = Some("Многоканальная карта".to_string());
        settings.audio.tracks = vec![
            track(None, 0, "judge"),          // канал 0 общего устройства
            track(None, 1, "defense"),        // канал 1 общего устройства
            track(None, 0, "room"),           // ДУБЛЬ канала 0 на другой дорожке
        ];
        let tracks = resolve_tracks(&settings).unwrap();
        let params = build_monitor_params(&settings, std::path::Path::new("/tmp/root"), &tracks);

        assert_eq!(params.len(), 3, "поток на каждую дорожку карты");
        let ids: Vec<u32> = params.iter().map(|p| p.track_id).collect();
        assert_eq!(ids, vec![0, 1, 2], "эмиссия под всеми track_id");
        assert_eq!(params[0].channel_index, Some(0));
        assert_eq!(params[1].channel_index, Some(1));
        // Дубль: третья дорожка — то же устройство и канал 0, но свой track_id.
        assert_eq!(params[2].channel_index, Some(0));
        assert_eq!(params[2].track_id, 2);
        for p in &params {
            assert_eq!(p.target_channels, 1, "дорожка моно");
            assert_eq!(p.device.as_deref(), Some("Многоканальная карта"));
        }
    }
}
