//! Tauri-команды встроенного проигрывателя (этап 10.1 —
//! `promts/10_1_playback.md`, шаг 3).
//!
//! Тонкая обвязка над `crate::player` (таймлайн/декодер/FSM — Tauri-agnostic,
//! тестируется отдельно) + `store::manifest` (те же паттерны, что
//! `query_cmds`/`sync_cmds`: реконсиляция каталога, чтение сегментов/дорожек/
//! разметки). Вывод звука — `rodio` поверх отдельного от записи
//! аудио-устройства: `PlayerState` — собственный Tauri-managed мьютекс,
//! независимый от `CaptureState`/`MonitorState`, поэтому идущая запись не
//! страдает от параллельного прослушивания (разные устройства, разные
//! потоки, разные блокировки).
//!
//! Команды: `player_open_session`, `player_select_track`, `player_play`,
//! `player_pause`, `player_seek`, `player_set_rate`, `player_set_volume`,
//! `player_close`. Событие `player_position` — позиция воспроизведения для UI
//! (частота — `player.position_update_hz`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use rodio::{OutputStream, OutputStreamHandle, Sink};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::audio::tracks::SINGLE_TRACK_ROLE;
use crate::integrity::annotations::{self, AnnotationSnapshot, MarkerState, RoleSpanState};
use crate::ipc::audio_cmds::operator_identity;
use crate::ipc::{load_settings, resolve_storage_root, MANIFEST_FILE};
use crate::player::audit;
use crate::player::source::{self, PlayerError};
use crate::player::state::{PlayerEvent, PlayerMachine, PlayerState as MachineState};
use crate::player::timeline::Timeline;
use crate::reliability::watchdog::now_unix_ms;
use crate::store::manifest::ManifestStore;
use crate::store::reconcile;

/// Одна дорожка плейбека: роль/метка (для UI) + собственный таймлайн.
struct TrackEntry {
    track_id: u32,
    role: String,
    label: String,
    /// Число каналов дорожки (легаси одноканальная сессия — `audio.channels`
    /// сессии; реальная многоканальная дорожка, этап 09, — всегда моно).
    channels: u16,
    timeline: Timeline,
}

/// Выбор источника звука: конкретная дорожка или сведённый микс.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrackSelector {
    Track { track_id: u32 },
    Mix,
}

/// Цель перемотки: абсолютное время сессии или метка/интервал по `id`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SeekTarget {
    Ms { ms: u64 },
    Marker { id: String },
}

/// Живой движок воспроизведения: держит `Sink` (управляет play/pause/
/// скоростью/громкостью выбранного источника) и счётчик реально проигранных
/// фреймов для событий позиции (см. `player::source::PositionTrackingSource`).
struct Engine {
    sink: Sink,
    played_frames: Arc<AtomicU64>,
    /// Длина выбранного источника (дорожки/микса) в фреймах — для «конца» и
    /// длительности в событии позиции.
    total_frames: u64,
}

/// Открытая в проигрывателе сессия.
struct ActivePlayback {
    sample_rate_hz: u32,
    key: Option<[u8; 32]>,
    tracks: Vec<TrackEntry>,
    selector: TrackSelector,
    machine: PlayerMachine,
    session_started_at_unix_ms: u64,
    annotations: AnnotationSnapshot,
    /// Абсолютный фрейм (ось выбранной дорожки/микса), с которого начнётся
    /// следующее воспроизведение — обновляется сиком/сменой дорожки.
    seek_base_frame: u64,
    volume: f32,
    rate: f32,
    /// Открытое аудио-устройство вывода — держим, пока сессия открыта, чтобы
    /// не переоткрывать его на каждый play/seek. Сброс сессии → устройство
    /// освобождается.
    output: Option<DeviceThread>,
    engine: Option<Engine>,
    /// Флаг остановки текущего потока-эмиттера позиции (независим от
    /// жизненного цикла `engine`: живёт, пока идёт активное воспроизведение).
    emitter_stop: Option<Arc<AtomicBool>>,
}

/// Управляемое Tauri состояние проигрывателя. Один активный плейбек на
/// станцию: открытие новой сессии останавливает предыдущую.
#[derive(Default)]
pub struct PlayerState(Mutex<Option<ActivePlayback>>);

/// Открытое аудио-устройство вывода. `rodio::OutputStream` (как и
/// `cpal::Stream`, см. `audio::capture::capture_thread`) не `Send` на части
/// платформ — держим его в собственном выделенном потоке на всё время жизни
/// устройства, наружу отдаём только `Send`-безопасный `OutputStreamHandle`
/// (по нему создаются `Sink`). Drop `_keep_alive` (сессия закрыта/сброшена) —
/// сигнал потоку завершиться и освободить устройство.
struct DeviceThread {
    handle: OutputStreamHandle,
    _keep_alive: mpsc::Sender<()>,
}

/// Открыть устройство вывода по умолчанию на выделенном потоке.
fn open_output_device() -> Result<DeviceThread, String> {
    let (handle_tx, handle_rx) = mpsc::channel::<Result<OutputStreamHandle, String>>();
    let (keep_alive_tx, keep_alive_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        let stream = match OutputStream::try_default() {
            Ok((stream, handle)) => {
                if handle_tx.send(Ok(handle)).is_err() {
                    return; // вызывающий уже не ждёт (ошибка/таймаут выше)
                }
                stream
            }
            Err(e) => {
                let _ = handle_tx.send(Err(e.to_string()));
                return;
            }
        };
        // Держим stream живым, пока не отсоединится (Drop) отправитель
        // keep_alive_tx — тогда recv() вернёт Err и поток завершится,
        // освободив устройство.
        let _ = keep_alive_rx.recv();
        drop(stream);
    });
    let handle = handle_rx
        .recv()
        .map_err(|_| "поток вывода звука не запустился".to_string())??;
    Ok(DeviceThread {
        handle,
        _keep_alive: keep_alive_tx,
    })
}

/// Дорожка в ответе `player_open_session` (для UI: список/выбор дорожки).
#[derive(Debug, Clone, Serialize)]
pub struct TrackView {
    pub track_id: u32,
    pub role: String,
    pub label: String,
}

/// Ответ открытия сессии в проигрывателе.
#[derive(Debug, Clone, Serialize)]
pub struct PlayerSessionInfo {
    pub session_id: String,
    /// Время старта сессии (мс от эпохи Unix) — для отображения даты/времени
    /// заседания в шапке экрана.
    pub started_at_unix_ms: u64,
    /// Привязка к делу (`store::case_binding`), если была сделана; `None` —
    /// запись без привязки.
    pub adjudication_ref: Option<String>,
    pub tracks: Vec<TrackView>,
    pub markers: Vec<MarkerState>,
    pub role_spans: Vec<RoleSpanState>,
    pub duration_ms: u64,
    pub sample_rate_hz: u32,
    /// Целостность сессии (все сегменты хешированы) — статус для UI.
    pub integrity_ok: bool,
}

/// Полезная нагрузка события `player_position`.
#[derive(Debug, Clone, Serialize)]
struct PlayerPositionEvent {
    position_ms: u64,
    duration_ms: u64,
    /// `playing` / `paused` / `stopped` (сессия открыта, но не играет —
    /// в т.ч. сразу после открытия/сика без предшествующего play).
    state: &'static str,
}

const EVENT_PLAYER_POSITION: &str = "player_position";
/// Событие закрытия сессии проигрывателя — чтобы внешние индикаторы (компакт-
/// оверлей, этап 10.5) узнали, что воспроизведение больше не активно (событие
/// `player_position` о закрытии не сообщает).
const EVENT_PLAYER_CLOSED: &str = "player_closed";

/// Снимок состояния проигрывателя для окон/индикаторов, открывшихся уже во время
/// воспроизведения (этап 10.5). `player_position` эмитится только во время игры/
/// сика — новый слушатель без этого снимка не знал бы текущее состояние.
#[derive(Debug, Clone, Serialize)]
pub struct PlayerStatusView {
    /// Открыта ли сессия в проигрывателе (есть чем управлять).
    pub active: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// `playing` / `paused` / `stopped` (см. `PlayerPositionEvent`).
    pub state: &'static str,
}

/// Текущее состояние проигрывателя (для восстановления статуса в окне/оверлее,
/// открывшемся во время воспроизведения). Сессия не открыта → `active=false`.
#[tauri::command]
pub fn player_status(state: State<'_, PlayerState>) -> Result<PlayerStatusView, String> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "состояние плеера повреждено".to_string())?;
    let Some(active) = guard.as_ref() else {
        return Ok(PlayerStatusView {
            active: false,
            position_ms: 0,
            duration_ms: 0,
            state: "stopped",
        });
    };
    let played = active
        .engine
        .as_ref()
        .map(|e| e.played_frames.load(Ordering::Relaxed))
        .unwrap_or(0);
    let total_frames = active
        .engine
        .as_ref()
        .map(|e| e.total_frames)
        .unwrap_or_else(|| selected_total_frames(active));
    Ok(PlayerStatusView {
        active: true,
        position_ms: frames_to_ms(
            active.seek_base_frame.saturating_add(played),
            active.sample_rate_hz,
        ),
        duration_ms: frames_to_ms(total_frames, active.sample_rate_hz),
        state: playing_state_code(active.machine.state()),
    })
}

/// Открыть сессию `dir` в проигрывателе: реконсилировать манифест, собрать
/// таймлайны дорожек, резолвить ключ шифрования (если включено), загрузить
/// разметку (метки/интервалы ролей) и зафиксировать аудит-событие доступа.
/// Заменяет предыдущий активный плейбек станции, если был.
#[tauri::command]
pub fn player_open_session(
    app: AppHandle,
    state: State<'_, PlayerState>,
    dir: String,
) -> Result<PlayerSessionInfo, String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;
    let store = ManifestStore::open(&root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;
    let key = crate::ipc::station_key_for_read(&settings, &root);
    let session_id = reconcile::reconcile_session(&store, &PathBuf::from(&dir), key.as_ref())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("каталог не содержит начатой сессии: {dir}"))?;

    let session = store
        .get_session(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("сессия {session_id} не найдена"))?;

    let all_segments = store.get_segments(&session_id).map_err(|e| e.to_string())?;
    let track_records = store.get_tracks(&session_id).map_err(|e| e.to_string())?;

    let tracks: Vec<TrackEntry> = if track_records.is_empty() {
        // Легаси/v1: единственная дорожка track_id=0 (как в store::export).
        vec![TrackEntry {
            track_id: 0,
            role: SINGLE_TRACK_ROLE.to_string(),
            label: "Запись".to_string(),
            channels: session.channels,
            timeline: Timeline::build(&all_segments, 0, session.sample_rate_hz),
        }]
    } else {
        track_records
            .iter()
            .map(|t| TrackEntry {
                track_id: t.track_id,
                role: t.role.clone(),
                label: t.label.clone(),
                // Многоканальные дорожки (этап 09) — всегда моно.
                channels: 1,
                timeline: Timeline::build(&all_segments, t.track_id, session.sample_rate_hz),
            })
            .collect()
    };

    if tracks.iter().all(|t| t.timeline.total_frames == 0) {
        return Err("сессия не содержит записанных сегментов".to_string());
    }

    let annotation_log = store.get_annotations(&session_id).map_err(|e| e.to_string())?;
    let snapshot = annotations::fold(&annotation_log);

    let segments_hashed = all_segments.iter().filter(|s| !s.sha256.is_empty()).count();
    let integrity_ok = !all_segments.is_empty() && segments_hashed == all_segments.len();

    let default_selector = TrackSelector::Track {
        track_id: tracks[0].track_id,
    };
    let duration_ms = frames_to_ms(tracks[0].timeline.total_frames, session.sample_rate_hz);

    let info = PlayerSessionInfo {
        session_id: session_id.clone(),
        started_at_unix_ms: session.started_at_unix_ms,
        adjudication_ref: session.adjudication_ref.clone(),
        tracks: tracks
            .iter()
            .map(|t| TrackView {
                track_id: t.track_id,
                role: t.role.clone(),
                label: t.label.clone(),
            })
            .collect(),
        markers: snapshot.markers.clone(),
        role_spans: snapshot.role_spans.clone(),
        duration_ms,
        sample_rate_hz: session.sample_rate_hz,
        integrity_ok,
    };

    // Аудит доступа (deliverable 4): сразу при открытии — самый ранний и
    // однозначный момент, когда содержимое сессии становится доступно UI.
    audit::record_access(&store, &session_id, &operator_identity(&app), now_unix_ms())
        .map_err(|e| e.to_string())?;

    let mut machine = PlayerMachine::new();
    machine
        .apply(PlayerEvent::Open)
        .map_err(|e| e.to_string())?;

    let active = ActivePlayback {
        sample_rate_hz: session.sample_rate_hz,
        key,
        tracks,
        selector: default_selector,
        machine,
        session_started_at_unix_ms: session.started_at_unix_ms,
        annotations: snapshot,
        seek_base_frame: 0,
        volume: 1.0,
        rate: 1.0,
        output: None,
        engine: None,
        emitter_stop: None,
    };

    let mut guard = state
        .0
        .lock()
        .map_err(|_| "состояние плеера повреждено".to_string())?;
    if let Some(mut prev) = guard.take() {
        teardown(&mut prev);
    }
    *guard = Some(active);

    // Снимок позиции для внешних индикаторов (компакт-оверлей): сообщает, что
    // сессия открыта, её длительность и позицию 0 (ещё не играли).
    if let Some(a) = guard.as_ref() {
        emit_position_now(&app, a);
    }

    Ok(info)
}

/// Выбрать источник звука: конкретная дорожка или сведённый микс. Сбрасывает
/// текущее воспроизведение и позицию на 0 (простое предсказуемое поведение).
#[tauri::command]
pub fn player_select_track(
    app: AppHandle,
    state: State<'_, PlayerState>,
    selector: TrackSelector,
) -> Result<(), String> {
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "состояние плеера повреждено".to_string())?;
    let active = guard
        .as_mut()
        .ok_or_else(|| "сессия не открыта".to_string())?;
    if let TrackSelector::Track { track_id } = selector {
        if !active.tracks.iter().any(|t| t.track_id == track_id) {
            return Err(format!("дорожка {track_id} не найдена"));
        }
    }
    teardown(active);
    active.selector = selector;
    active.seek_base_frame = 0;
    emit_position_now(&app, active);
    Ok(())
}

/// Начать/возобновить воспроизведение с текущей позиции.
#[tauri::command]
pub fn player_play(app: AppHandle, state: State<'_, PlayerState>) -> Result<(), String> {
    let settings = load_settings(&app)?;
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "состояние плеера повреждено".to_string())?;
    let active = guard
        .as_mut()
        .ok_or_else(|| "сессия не открыта".to_string())?;

    if active.machine.state() != MachineState::Playing {
        active
            .machine
            .apply(PlayerEvent::Play)
            .map_err(|e| e.to_string())?;
    }
    start_playback(active, &app, settings.player.position_update_hz)
}

/// Приостановить воспроизведение (позиция сохраняется).
#[tauri::command]
pub fn player_pause(app: AppHandle, state: State<'_, PlayerState>) -> Result<(), String> {
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "состояние плеера повреждено".to_string())?;
    let active = guard
        .as_mut()
        .ok_or_else(|| "сессия не открыта".to_string())?;
    if let Some(engine) = &active.engine {
        engine.sink.pause();
    }
    stop_emitter(active);
    if active.machine.state() == MachineState::Playing {
        active
            .machine
            .apply(PlayerEvent::Pause)
            .map_err(|e| e.to_string())?;
    }
    // Пауза не эмитит периодических событий — сообщаем новое состояние сразу,
    // чтобы внешние индикаторы (компакт-оверлей) не «застряли» на playing.
    emit_position_now(&app, active);
    Ok(())
}

/// Перемотать к абсолютному времени сессии или к метке/интервалу по `id`
/// (семпл-точное смещение — `player::timeline::Timeline::frame_for_marker`).
/// Play/pause-состояние сохраняется: если играло — продолжает играть с новой
/// позиции.
#[tauri::command]
pub fn player_seek(
    app: AppHandle,
    state: State<'_, PlayerState>,
    to: SeekTarget,
) -> Result<(), String> {
    let settings = load_settings(&app)?;
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "состояние плеера повреждено".to_string())?;
    let active = guard
        .as_mut()
        .ok_or_else(|| "сессия не открыта".to_string())?;

    let target_frame = resolve_seek_frame(active, &to)?;
    let was_playing = active.machine.state() == MachineState::Playing;

    teardown(active);
    active.seek_base_frame = target_frame;
    active
        .machine
        .apply(PlayerEvent::Seek)
        .map_err(|e| e.to_string())?;

    if was_playing {
        start_playback(active, &app, settings.player.position_update_hz)?;
    }
    // Немедленный эмит: пока не играет (или пока свежий эмиттер не сделал
    // первый тик), UI иначе не узнает о новой позиции после сика.
    emit_position_now(&app, active);
    Ok(())
}

/// Установить скорость воспроизведения (валидируется по `player.playback_rates`).
#[tauri::command]
pub fn player_set_rate(app: AppHandle, state: State<'_, PlayerState>, rate: f32) -> Result<(), String> {
    let settings = load_settings(&app)?;
    if !settings
        .player
        .playback_rates
        .iter()
        .any(|r| (*r - rate).abs() < f32::EPSILON)
    {
        return Err(format!(
            "скорость {rate} вне списка настройки player.playback_rates"
        ));
    }
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "состояние плеера повреждено".to_string())?;
    let active = guard
        .as_mut()
        .ok_or_else(|| "сессия не открыта".to_string())?;
    active.rate = rate;
    if let Some(engine) = &active.engine {
        engine.sink.set_speed(rate);
    }
    Ok(())
}

/// Установить громкость (`0.0..=1.0`).
#[tauri::command]
pub fn player_set_volume(state: State<'_, PlayerState>, volume: f32) -> Result<(), String> {
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "состояние плеера повреждено".to_string())?;
    let active = guard
        .as_mut()
        .ok_or_else(|| "сессия не открыта".to_string())?;
    active.volume = volume.clamp(0.0, 1.0);
    if let Some(engine) = &active.engine {
        engine.sink.set_volume(active.volume);
    }
    Ok(())
}

/// Закрыть сессию в проигрывателе (уход с экрана) — останавливает
/// воспроизведение и освобождает аудио-устройство вывода.
#[tauri::command]
pub fn player_close(app: AppHandle, state: State<'_, PlayerState>) -> Result<(), String> {
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "состояние плеера повреждено".to_string())?;
    if let Some(mut active) = guard.take() {
        teardown(&mut active);
    }
    // Оповестить внешние индикаторы (компакт-оверлей), что управлять больше
    // нечем — событие позиции о закрытии не сообщает.
    let _ = app.emit(EVENT_PLAYER_CLOSED, ());
    Ok(())
}

/// Остановить эмиттер позиции (если запущен) — независим от `engine`, живёт
/// только пока идёт активное воспроизведение.
fn stop_emitter(active: &mut ActivePlayback) {
    if let Some(stop) = active.emitter_stop.take() {
        stop.store(true, Ordering::Release);
    }
}

/// Остановить воспроизведение и освободить движок/устройство. Сика/смены
/// дорожки/закрытия сессии — общий путь: дальнейшее решение (пересобрать
/// заново или оставить закрытым) принимает вызывающая команда.
fn teardown(active: &mut ActivePlayback) {
    stop_emitter(active);
    active.engine = None; // drop Sink — звук останавливается немедленно.
    active.output = None; // drop OutputStream — устройство освобождается.
}

/// Выбранные (по `selector`) дорожки.
fn selected_tracks(active: &ActivePlayback) -> Vec<&TrackEntry> {
    match active.selector {
        TrackSelector::Track { track_id } => active
            .tracks
            .iter()
            .filter(|t| t.track_id == track_id)
            .collect(),
        TrackSelector::Mix => active.tracks.iter().collect(),
    }
}

/// Длина выбранного источника в фреймах: для одной дорожки — её длина, для
/// микса — длина кратчайшей (см. `player::source::MixSource`).
fn selected_total_frames(active: &ActivePlayback) -> u64 {
    selected_tracks(active)
        .iter()
        .map(|t| t.timeline.total_frames)
        .min()
        .unwrap_or(0)
}

/// Построить движок воспроизведения (устройство при необходимости, `Sink`,
/// источник от `active.seek_base_frame` по текущему `selector`).
fn build_engine(active: &mut ActivePlayback) -> Result<(), String> {
    if active.output.is_none() {
        active.output = Some(open_output_device()?);
    }
    let handle = &active
        .output
        .as_ref()
        .expect("устройство открыто выше")
        .handle;
    let sink = Sink::try_new(handle).map_err(|e| e.to_string())?;

    let total_frames = selected_total_frames(active);
    let played_frames = Arc::new(AtomicU64::new(0));

    let boxed: Box<dyn rodio::Source<Item = f32> + Send> = match active.selector {
        TrackSelector::Track { track_id } => {
            let track = active
                .tracks
                .iter()
                .find(|t| t.track_id == track_id)
                .ok_or_else(|| format!("дорожка {track_id} не найдена"))?;
            let src = source::seek_source(
                &track.timeline,
                active.seek_base_frame,
                track.channels,
                active.key,
            )
            .map_err(player_error_to_string)?;
            Box::new(source::PositionTrackingSource::new(
                src,
                Arc::clone(&played_frames),
                track.channels,
            ))
        }
        TrackSelector::Mix => {
            let mut parts = Vec::with_capacity(active.tracks.len());
            for t in &active.tracks {
                let src = source::seek_source(&t.timeline, active.seek_base_frame, t.channels, active.key)
                    .map_err(player_error_to_string)?;
                parts.push(src);
            }
            let mix = source::MixSource::new(parts);
            Box::new(source::PositionTrackingSource::new(
                mix,
                Arc::clone(&played_frames),
                1,
            ))
        }
    };

    sink.set_volume(active.volume);
    sink.set_speed(active.rate);
    sink.append(boxed);

    active.engine = Some(Engine {
        sink,
        played_frames,
        total_frames,
    });
    Ok(())
}

fn player_error_to_string(e: PlayerError) -> String {
    e.to_string()
}

/// Убедиться, что движок построен и играет, (пере)запустить эмиттер позиции.
fn start_playback(
    active: &mut ActivePlayback,
    app: &AppHandle,
    position_update_hz: u32,
) -> Result<(), String> {
    if active.engine.is_none() {
        build_engine(active)?;
    }
    stop_emitter(active);
    let engine = active.engine.as_ref().expect("построен выше");
    engine.sink.play();

    let stop = Arc::new(AtomicBool::new(false));
    active.emitter_stop = Some(Arc::clone(&stop));
    spawn_position_emitter(
        app.clone(),
        Arc::clone(&engine.played_frames),
        stop,
        active.seek_base_frame,
        engine.total_frames,
        active.sample_rate_hz,
        position_update_hz,
    );
    Ok(())
}

/// Абсолютный фрейм цели сика на оси выбранного источника.
fn resolve_seek_frame(active: &ActivePlayback, to: &SeekTarget) -> Result<u64, String> {
    match to {
        SeekTarget::Ms { ms } => Ok(ms.saturating_mul(active.sample_rate_hz as u64) / 1000),
        SeekTarget::Marker { id } => {
            let offset_ms = active
                .annotations
                .markers
                .iter()
                .find(|m| &m.id == id)
                .map(|m| m.offset_ms)
                .or_else(|| {
                    active
                        .annotations
                        .role_spans
                        .iter()
                        .find(|s| &s.id == id)
                        .map(|s| s.start_offset_ms)
                })
                .ok_or_else(|| format!("метка/интервал {id} не найдены"))?;
            let timeline = &active
                .tracks
                .first()
                .ok_or_else(|| "нет дорожек".to_string())?
                .timeline;
            Ok(timeline.frame_for_marker(active.session_started_at_unix_ms, offset_ms))
        }
    }
}

/// Фоновый поток, эмитящий `player_position` c периодом
/// `1000/position_update_hz` мс, пока не остановлен (`stop`) или пока
/// выбранный источник не закончился.
fn spawn_position_emitter(
    app: AppHandle,
    played_frames: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    seek_base_frame: u64,
    total_frames: u64,
    sample_rate_hz: u32,
    position_update_hz: u32,
) {
    let interval = Duration::from_millis(1000 / position_update_hz.max(1) as u64);
    thread::spawn(move || loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        thread::sleep(interval);
        if stop.load(Ordering::Acquire) {
            return;
        }
        let remaining_total = total_frames.saturating_sub(seek_base_frame);
        let played = played_frames.load(Ordering::Relaxed).min(remaining_total);
        let ended = played >= remaining_total;
        let position_ms = frames_to_ms(seek_base_frame.saturating_add(played), sample_rate_hz);
        let duration_ms = frames_to_ms(total_frames, sample_rate_hz);
        let _ = app.emit(
            EVENT_PLAYER_POSITION,
            PlayerPositionEvent {
                position_ms,
                duration_ms,
                state: if ended { "stopped" } else { "playing" },
            },
        );
        if ended {
            return;
        }
    });
}

fn frames_to_ms(frames: u64, sample_rate_hz: u32) -> u64 {
    if sample_rate_hz == 0 {
        return 0;
    }
    frames * 1000 / sample_rate_hz as u64
}

/// Строковый код состояния для `player_position` (см. doc `PlayerPositionEvent`).
fn playing_state_code(state: MachineState) -> &'static str {
    match state {
        MachineState::Playing => "playing",
        MachineState::Paused => "paused",
        MachineState::Idle | MachineState::Stopped => "stopped",
    }
}

/// Эмитить текущую позицию немедленно (вне периодического эмиттера) —
/// нужно после сика/открытия, когда воспроизведение не идёт (эмиттер не
/// запущен) и UI иначе не узнает о новой позиции до следующего play.
fn emit_position_now(app: &AppHandle, active: &ActivePlayback) {
    let played = active
        .engine
        .as_ref()
        .map(|e| e.played_frames.load(Ordering::Relaxed))
        .unwrap_or(0);
    let total_frames = active
        .engine
        .as_ref()
        .map(|e| e.total_frames)
        .unwrap_or_else(|| selected_total_frames(active));
    let position_ms = frames_to_ms(active.seek_base_frame.saturating_add(played), active.sample_rate_hz);
    let duration_ms = frames_to_ms(total_frames, active.sample_rate_hz);
    let _ = app.emit(
        EVENT_PLAYER_POSITION,
        PlayerPositionEvent {
            position_ms,
            duration_ms,
            state: playing_state_code(active.machine.state()),
        },
    );
}
