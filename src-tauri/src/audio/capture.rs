//! Конвейер захвата (этап 01 — `promts/01_audio_core.md`, шаги 4, 6) +
//! интеграция надёжности (этап 02 — `promts/02_recorder_reliability.md`).
//!
//! `cpal` input stream → [`ring`] (producer в аудио-callback'е) → consumer-поток
//! → [`SegmentWriter`]. Аудио-callback **лёгкий**: приведение sample-формата к
//! `f32`, запись в кольцевой буфер и удар [`Heartbeat`] (живость для watchdog);
//! нормализация, расчёт уровней и запись на диск — в consumer-потоке.
//!
//! Этап 02 добавляет инъектируемые крючки надёжности ([`ReliabilityConfig`]):
//! - **журнал** (write-ahead): consumer пишет `SegmentCompleted` по факту
//!   закрытия сегмента; lifecycle-записи (`SessionStarted`/`Stopped`) — за IPC;
//! - **зеркало** сегментов (best-effort);
//! - **контроль диска**: при `Low` — предупреждение, при `Critical` — корректный
//!   стоп с гарантированным флашем (решение заказчика);
//! - **предупреждение о длине сессии** (`max_session_hours`) — в v1 только
//!   событие, запись продолжается;
//! - **supervisor-поток**: watchdog (по [`Heartbeat`]) перезапускает зависший
//!   захват; монитор устройства ставит на паузу при обрыве и авто-возобновляет
//!   при возврате (если включено).
//!
//! Все параметры — из `Settings` (реестр `docs/configuration.md`); магических
//! чисел нет: единый интервал контроля надёжности — `watchdog_timeout_ms`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, FromSample, Sample, SampleFormat, SampleRate, SizedSample, Stream, StreamConfig,
};
use serde::Serialize;

use super::{convert, ring, AudioError};
use crate::recorder::journal::{Journal, JournalRecord};
use crate::recorder::segment_writer::{SegmentConfig, SegmentInfo, SegmentWriter};
use crate::reliability::device_monitor::{DeviceEvent, DeviceMonitor};
use crate::reliability::disk_monitor::{self, DiskStatus, DiskThresholds};
use crate::reliability::mirror::Mirror;
use crate::reliability::watchdog::{is_stalled, now_unix_ms, Heartbeat};

/// Фактический формат захвата устройства (после подбора конфигурации).
#[derive(Debug, Clone, Copy)]
pub struct NativeFormat {
    pub sample_rate_hz: u32,
    pub channels: u16,
}

/// Событие индикатора уровня (RMS/пик нормированного сигнала, `[0.0, 1.0]`).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LevelEvent {
    pub peak: f32,
    pub rms: f32,
}

/// Колбэк уровней (IPC подключает к нему Tauri-эмиттер `audio_level`).
pub type LevelCallback = Box<dyn Fn(LevelEvent) + Send + 'static>;

/// Событие надёжности для UI/журнала (этап 02). Тег `kind` (`snake_case`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReliabilityEvent {
    /// Свободное место ниже порога предупреждения.
    DiskLow { free_mb: u64 },
    /// Критический порог свободного места — выполнен защитный стоп.
    DiskCritical { free_mb: u64 },
    /// Watchdog перезапустил зависший захват.
    WatchdogRestart,
    /// Устройство пропало — запись на паузе.
    DeviceLost,
    /// Устройство вернулось — запись возобновлена (при авто-резюме).
    DeviceBack,
    /// Достигнут `max_session_hours` (запись продолжается).
    MaxDurationWarning,
}

/// Колбэк событий надёжности; `Arc` — разделяется consumer- и supervisor-потоками.
pub type ReliabilityCallback = Arc<dyn Fn(ReliabilityEvent) + Send + Sync + 'static>;

/// Параметры контроля диска (путь тома + пороги из `Settings.reliability`).
#[derive(Debug, Clone)]
pub struct DiskWatch {
    /// Путь, по тому которого замеряется свободное место (корень хранилища).
    pub path: PathBuf,
    pub thresholds: DiskThresholds,
}

/// Параметры запуска захвата (заполняются из `Settings` в слое IPC).
#[derive(Debug, Clone)]
pub struct CaptureParams {
    /// `audio.device` — `None` означает системное устройство по умолчанию.
    pub device: Option<String>,
    /// `audio.sample_rate_hz` — желаемая частота (иначе берётся нативная).
    pub desired_sample_rate_hz: u32,
    /// `audio.channels` — целевое число каналов (downmix к нему).
    pub target_channels: u16,
    /// `audio.bit_depth` — разрядность PCM.
    pub bit_depth: u16,
    /// `audio.capture_buffer_seconds` — глубина кольцевого буфера.
    pub capture_buffer_seconds: f32,
    /// `audio.level_update_hz` — частота событий уровня.
    pub level_update_hz: u32,
    /// `recorder.segment_seconds` — длина сегмента.
    pub segment_seconds: u32,
    /// `recorder.flush_interval_ms` — интервал fsync.
    pub flush_interval_ms: u32,
    /// Каталог сессии для сегментов (резолвится из `storage.root_path`).
    pub output_dir: PathBuf,
}

/// Конфигурация надёжности захвата (этап 02). Все поля опциональны: `none()`
/// воспроизводит поведение этапа 01 (используется в тестах конвейера).
pub struct ReliabilityConfig {
    /// Общий журнал сессии (lifecycle-записи делает IPC; сегменты — consumer).
    pub journal: Option<Arc<Mutex<Journal>>>,
    /// Каталог зеркала (`reliability.mirror.path`), если включено.
    pub mirror_dir: Option<PathBuf>,
    /// Контроль свободного места (пороги из реестра).
    pub disk: Option<DiskWatch>,
    /// Порог длины сессии (`max_session_hours`) — предупреждение.
    pub max_session: Option<Duration>,
    /// Таймаут/интервал контроля надёжности (`watchdog_timeout_ms`). `None`
    /// отключает supervisor-поток (watchdog + монитор устройства).
    pub watchdog_timeout: Option<Duration>,
    /// `reliability.device_reconnect.auto_resume`.
    pub auto_resume: bool,
    /// Имя устройства для пробника присутствия (как `CaptureParams.device`).
    pub device_name: Option<String>,
    /// Колбэк событий надёжности (UI + журнал на стороне IPC).
    pub on_event: Option<ReliabilityCallback>,
}

impl ReliabilityConfig {
    /// Конфигурация без надёжности (поведение этапа 01).
    pub fn none() -> Self {
        Self {
            journal: None,
            mirror_dir: None,
            disk: None,
            max_session: None,
            watchdog_timeout: None,
            auto_resume: false,
            device_name: None,
            on_event: None,
        }
    }
}

/// Крючки надёжности для consumer-потока (подмножество [`ReliabilityConfig`]).
pub struct ConsumerReliability {
    pub journal: Option<Arc<Mutex<Journal>>>,
    pub mirror: Option<Mirror>,
    pub disk: Option<DiskWatch>,
    pub max_session: Option<Duration>,
    pub on_event: Option<ReliabilityCallback>,
}

impl ConsumerReliability {
    /// Без надёжности (используется тестами конвейера).
    pub fn none() -> Self {
        Self {
            journal: None,
            mirror: None,
            disk: None,
            max_session: None,
            on_event: None,
        }
    }

    fn emit(&self, ev: ReliabilityEvent) {
        if let Some(cb) = &self.on_event {
            cb(ev);
        }
    }

    fn journal(&self, rec: &JournalRecord) {
        if let Some(j) = &self.journal {
            if let Ok(mut g) = j.lock() {
                let _ = g.append(rec);
            }
        }
    }
}

/// Конфигурация consumer-потока (нормализация + сегментная запись).
#[derive(Debug, Clone)]
pub struct ConsumerConfig {
    pub native_channels: u16,
    pub target_channels: u16,
    pub bit_depth: u16,
    pub sample_rate_hz: u32,
    pub segment_seconds: u32,
    pub flush_interval: Duration,
    pub level_update_hz: u32,
    pub output_dir: PathBuf,
    /// Размер рабочего буфера чтения (= ёмкость кольцевого буфера).
    pub scratch_len: usize,
}

/// Команды потоку захвата.
enum ControlCmd {
    Pause,
    Resume,
    /// Перезапуск стрима (watchdog): пауза + воспроизведение «вживую».
    Rekick,
    Stop,
}

/// Активная сессия захвата: владеет потоком стрима, consumer- и supervisor-
/// потоками.
pub struct CaptureSession {
    cmd_tx: Sender<ControlCmd>,
    capture_handle: Option<JoinHandle<()>>,
    writer_stop: Arc<AtomicBool>,
    writer_handle: Option<JoinHandle<Result<Vec<SegmentInfo>, AudioError>>>,
    native: NativeFormat,
    /// Флаг «на паузе» — разделяется с supervisor'ом, чтобы watchdog не
    /// перезапускал намеренно приостановленный (оператором) захват.
    paused: Arc<AtomicBool>,
    supervisor_stop: Arc<AtomicBool>,
    supervisor_handle: Option<JoinHandle<()>>,
}

impl CaptureSession {
    /// Запустить захват: подобрать устройство/конфиг, поднять `cpal`-поток,
    /// consumer-поток записи и (при наличии настроек) supervisor надёжности.
    pub fn start(
        params: CaptureParams,
        level_cb: LevelCallback,
        reliability: ReliabilityConfig,
    ) -> Result<Self, AudioError> {
        let (init_tx, init_rx) = mpsc::channel();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (err_tx, err_rx) = mpsc::channel::<()>();

        let heartbeat = Heartbeat::new();
        let hb_capture = heartbeat.clone();
        let thread_params = params.clone();
        let capture_handle = thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || capture_thread(thread_params, init_tx, cmd_rx, hb_capture, err_tx))
            .map_err(|e| AudioError::Stream(e.to_string()))?;

        // Ждём результат инициализации потока захвата.
        let (consumer, native) = init_rx
            .recv()
            .map_err(|_| AudioError::Stream("поток захвата не инициализировался".into()))??;

        let consumer_cfg = ConsumerConfig {
            native_channels: native.channels,
            target_channels: params.target_channels,
            bit_depth: params.bit_depth,
            sample_rate_hz: native.sample_rate_hz,
            segment_seconds: params.segment_seconds,
            flush_interval: Duration::from_millis(params.flush_interval_ms as u64),
            level_update_hz: params.level_update_hz,
            output_dir: params.output_dir.clone(),
            scratch_len: consumer.capacity(),
        };

        // Зеркало создаём заранее (best-effort): сбой подготовки не валит запись.
        let mirror = match &reliability.mirror_dir {
            Some(dir) => match Mirror::new(dir) {
                Ok(m) => Some(m),
                Err(e) => {
                    eprintln!("[reliability] не удалось подготовить зеркало {dir:?}: {e}");
                    None
                }
            },
            None => None,
        };

        let consumer_rel = ConsumerReliability {
            journal: reliability.journal.clone(),
            mirror,
            disk: reliability.disk.clone(),
            max_session: reliability.max_session,
            on_event: reliability.on_event.clone(),
        };

        let writer_stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&writer_stop);
        let writer_handle = thread::Builder::new()
            .name("audio-writer".into())
            .spawn(move || run_consumer(consumer, consumer_cfg, level_cb, stop_for_thread, consumer_rel))
            .map_err(|e| AudioError::Stream(e.to_string()))?;

        // Supervisor (watchdog + монитор устройства) — только если задан интервал.
        let paused = Arc::new(AtomicBool::new(false));
        let supervisor_stop = Arc::new(AtomicBool::new(false));
        let supervisor_handle = match reliability.watchdog_timeout {
            Some(timeout) => Some(spawn_supervisor(SupervisorCtx {
                cmd_tx: cmd_tx.clone(),
                heartbeat,
                err_rx,
                paused: Arc::clone(&paused),
                stop: Arc::clone(&supervisor_stop),
                timeout,
                auto_resume: reliability.auto_resume,
                device_name: reliability.device_name.clone(),
                journal: reliability.journal.clone(),
                on_event: reliability.on_event.clone(),
            })?),
            None => None,
        };

        Ok(Self {
            cmd_tx,
            capture_handle: Some(capture_handle),
            writer_stop,
            writer_handle: Some(writer_handle),
            native,
            paused,
            supervisor_stop,
            supervisor_handle,
        })
    }

    /// Фактический формат захвата (нативная частота/каналы устройства).
    pub fn native_format(&self) -> NativeFormat {
        self.native
    }

    /// Поставить захват на паузу (устройство остаётся открытым).
    pub fn pause(&self) -> Result<(), AudioError> {
        // Помечаем паузу до отправки команды: watchdog не должен принять
        // намеренную паузу за зависание и «оживить» стрим.
        self.paused.store(true, Ordering::Release);
        self.cmd_tx
            .send(ControlCmd::Pause)
            .map_err(|_| AudioError::Stream("поток захвата недоступен".into()))
    }

    /// Возобновить захват после паузы.
    pub fn resume(&self) -> Result<(), AudioError> {
        self.paused.store(false, Ordering::Release);
        self.cmd_tx
            .send(ControlCmd::Resume)
            .map_err(|_| AudioError::Stream("поток захвата недоступен".into()))
    }

    /// Остановить захват, дождаться слива остатка буфера и финализации
    /// сегментов; вернуть список записанных сегментов (заглушка манифеста).
    pub fn stop(mut self) -> Result<Vec<SegmentInfo>, AudioError> {
        self.shutdown_supervisor();
        // 1) Останавливаем стрим (прекращается производство в буфер).
        let _ = self.cmd_tx.send(ControlCmd::Stop);
        if let Some(h) = self.capture_handle.take() {
            let _ = h.join();
        }
        // 2) Просим consumer дописать остаток и финализировать.
        self.writer_stop.store(true, Ordering::Release);
        match self.writer_handle.take() {
            Some(h) => h
                .join()
                .map_err(|_| AudioError::Io("поток записи завершился аварийно".into()))?,
            None => Ok(Vec::new()),
        }
    }

    fn shutdown_supervisor(&mut self) {
        self.supervisor_stop.store(true, Ordering::Release);
        if let Some(h) = self.supervisor_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        // Если сессию уронили без stop(), всё равно корректно гасим потоки.
        self.shutdown_supervisor();
        let _ = self.cmd_tx.send(ControlCmd::Stop);
        if let Some(h) = self.capture_handle.take() {
            let _ = h.join();
        }
        self.writer_stop.store(true, Ordering::Release);
        if let Some(h) = self.writer_handle.take() {
            let _ = h.join();
        }
    }
}

/// Контекст supervisor-потока надёжности.
struct SupervisorCtx {
    cmd_tx: Sender<ControlCmd>,
    heartbeat: Heartbeat,
    err_rx: Receiver<()>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    timeout: Duration,
    auto_resume: bool,
    device_name: Option<String>,
    journal: Option<Arc<Mutex<Journal>>>,
    on_event: Option<ReliabilityCallback>,
}

/// Поднять supervisor-поток: watchdog по [`Heartbeat`] + монитор устройства.
///
/// **Честная оговорка (как в этапе 01):** реальный обрыв устройства и рестарт
/// `cpal` зависят от железа и ОС и проверяются ручной приёмкой (CI без
/// устройства). Логика принятия решений (зависание, переходы present↔absent,
/// авто-резюм) покрыта юнит-тестами в `reliability::*`.
fn spawn_supervisor(ctx: SupervisorCtx) -> Result<JoinHandle<()>, AudioError> {
    thread::Builder::new()
        .name("reliability-supervisor".into())
        .spawn(move || supervisor_loop(ctx))
        .map_err(|e| AudioError::Stream(e.to_string()))
}

fn supervisor_loop(ctx: SupervisorCtx) {
    let emit = |ev: ReliabilityEvent| {
        if let Some(cb) = &ctx.on_event {
            cb(ev);
        }
    };
    let journal = |rec: &JournalRecord| {
        if let Some(j) = &ctx.journal {
            if let Ok(mut g) = j.lock() {
                let _ = g.append(rec);
            }
        }
    };

    let device_name = ctx.device_name.clone();
    let mut monitor = DeviceMonitor::new(move || device_present(&device_name));
    let timeout_ms = ctx.timeout.as_millis() as u64;
    // Единый реестровый интервал контроля надёжности — `watchdog_timeout_ms`.
    let poll = ctx.timeout;
    let mut stall_fired = false;
    // Пауза, инициированная обрывом устройства (для корректного авто-резюма).
    let mut device_paused = false;

    while !ctx.stop.load(Ordering::Acquire) {
        // Дренируем сигналы ошибок стрима, чтобы канал не рос (быстрый признак
        // проблемы устройства; подтверждение — через пробник присутствия ниже).
        while ctx.err_rx.try_recv().is_ok() {}

        // Монитор устройства: пауза при обрыве, авто-резюм при возврате.
        if let Some(event) = monitor.poll() {
            match event {
                DeviceEvent::Lost => {
                    if !ctx.paused.load(Ordering::Acquire) {
                        ctx.paused.store(true, Ordering::Release);
                        device_paused = true;
                        let _ = ctx.cmd_tx.send(ControlCmd::Pause);
                        journal(&JournalRecord::DeviceLost);
                        emit(ReliabilityEvent::DeviceLost);
                    }
                }
                DeviceEvent::Back => {
                    if device_paused && ctx.auto_resume {
                        ctx.paused.store(false, Ordering::Release);
                        device_paused = false;
                        let _ = ctx.cmd_tx.send(ControlCmd::Resume);
                        journal(&JournalRecord::DeviceBack);
                        emit(ReliabilityEvent::DeviceBack);
                    }
                }
            }
        }

        // Watchdog: только если запись не на паузе (намеренную паузу не трогаем).
        if !ctx.paused.load(Ordering::Acquire) {
            if is_stalled(now_unix_ms(), ctx.heartbeat.last(), timeout_ms) {
                if !stall_fired {
                    let _ = ctx.cmd_tx.send(ControlCmd::Rekick);
                    journal(&JournalRecord::WatchdogRestart);
                    emit(ReliabilityEvent::WatchdogRestart);
                    stall_fired = true;
                }
            } else {
                stall_fired = false;
            }
        }

        thread::sleep(poll);
    }
}

/// Присутствует ли устройство среди устройств ввода (для пробника монитора).
fn device_present(name: &Option<String>) -> bool {
    let host = cpal::default_host();
    match name {
        Some(wanted) => host
            .input_devices()
            .map(|mut it| it.any(|d| d.name().map(|n| &n == wanted).unwrap_or(false)))
            .unwrap_or(false),
        None => host.default_input_device().is_some(),
    }
}

/// Поток захвата: владеет `cpal::Stream` (не `Send`), управляется командами.
fn capture_thread(
    params: CaptureParams,
    init_tx: Sender<Result<(ring::Consumer, NativeFormat), AudioError>>,
    cmd_rx: Receiver<ControlCmd>,
    heartbeat: Heartbeat,
    err_tx: Sender<()>,
) {
    let built = build_stream_and_ring(&params, heartbeat, err_tx);
    let (stream, consumer, native) = match built {
        Ok(v) => v,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };

    if let Err(e) = stream.play() {
        let _ = init_tx.send(Err(AudioError::Stream(e.to_string())));
        return;
    }
    if init_tx.send(Ok((consumer, native))).is_err() {
        return; // запросившая сторона исчезла
    }

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            ControlCmd::Pause => {
                let _ = stream.pause();
            }
            ControlCmd::Resume => {
                let _ = stream.play();
            }
            ControlCmd::Rekick => {
                // Перезапуск зависшего захвата: пауза + воспроизведение.
                let _ = stream.pause();
                let _ = stream.play();
            }
            ControlCmd::Stop => break,
        }
    }
    drop(stream);
}

/// Подобрать устройство/конфигурацию, поднять кольцевой буфер и `cpal`-поток.
fn build_stream_and_ring(
    params: &CaptureParams,
    heartbeat: Heartbeat,
    err_tx: Sender<()>,
) -> Result<(Stream, ring::Consumer, NativeFormat), AudioError> {
    let host = cpal::default_host();
    let device = pick_device(&host, &params.device)?;
    let supported = pick_config(
        &device,
        params.desired_sample_rate_hz,
        params.target_channels,
    )?;

    let native = NativeFormat {
        sample_rate_hz: supported.sample_rate().0,
        channels: supported.channels(),
    };
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.config();

    // Ёмкость кольцевого буфера = capture_buffer_seconds × частота × каналы.
    let capacity = ((params.capture_buffer_seconds as f64)
        * native.sample_rate_hz as f64
        * native.channels as f64)
        .ceil() as usize;
    let (producer, consumer) = ring::channel(capacity.max(1));

    let stream = match sample_format {
        SampleFormat::I16 => build_input_stream::<i16>(&device, &config, producer, heartbeat, err_tx),
        SampleFormat::U16 => build_input_stream::<u16>(&device, &config, producer, heartbeat, err_tx),
        SampleFormat::I32 => build_input_stream::<i32>(&device, &config, producer, heartbeat, err_tx),
        SampleFormat::F32 => build_input_stream::<f32>(&device, &config, producer, heartbeat, err_tx),
        other => Err(AudioError::UnsupportedFormat(format!("{other:?}"))),
    }?;

    Ok((stream, consumer, native))
}

fn pick_device(host: &cpal::Host, name: &Option<String>) -> Result<Device, AudioError> {
    match name {
        Some(wanted) => {
            let mut devices = host
                .input_devices()
                .map_err(|e| AudioError::Enumerate(e.to_string()))?;
            devices
                .find(|d| d.name().map(|n| &n == wanted).unwrap_or(false))
                .ok_or_else(|| AudioError::DeviceNotFound(wanted.clone()))
        }
        None => host.default_input_device().ok_or(AudioError::NoInputDevice),
    }
}

/// Подобрать конфигурацию: если устройство поддерживает желаемую частоту при
/// целевом числе каналов — берём её; иначе — нативную конфигурацию по умолчанию
/// (частота как есть, **без ресемпла**).
fn pick_config(
    device: &Device,
    desired_rate: u32,
    desired_channels: u16,
) -> Result<cpal::SupportedStreamConfig, AudioError> {
    if let Ok(ranges) = device.supported_input_configs() {
        for r in ranges {
            if r.channels() == desired_channels
                && r.min_sample_rate().0 <= desired_rate
                && desired_rate <= r.max_sample_rate().0
            {
                return Ok(r.with_sample_rate(SampleRate(desired_rate)));
            }
        }
    }
    device
        .default_input_config()
        .map_err(|e| AudioError::Config(e.to_string()))
}

/// Построить типизированный входной поток: callback приводит семплы к `f32`,
/// пишет в кольцевой буфер и бьёт [`Heartbeat`] (живость для watchdog). Ошибка
/// потока (в т.ч. отключение устройства) сигналится через `err_tx`.
fn build_input_stream<T>(
    device: &Device,
    config: &StreamConfig,
    producer: ring::Producer,
    heartbeat: Heartbeat,
    err_tx: Sender<()>,
) -> Result<Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut scratch: Vec<f32> = Vec::new();
    let err_fn = move |e| {
        eprintln!("[audio] ошибка входного потока cpal: {e}");
        // Сигнал supervisor'у (обрыв устройства и т.п.); приёмник может отсутствовать.
        let _ = err_tx.send(());
    };
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                heartbeat.beat();
                scratch.clear();
                scratch.extend(data.iter().map(|&s| f32::from_sample(s)));
                producer.push_slice(&scratch);
            },
            err_fn,
            None,
        )
        .map_err(|e| AudioError::Stream(e.to_string()))
}

/// Consumer-цикл: читает из кольцевого буфера, нормализует формат, считает
/// уровни (с троттлингом до `level_update_hz`), пишет сегменты и применяет
/// крючки надёжности (журнал сегментов, зеркало, контроль диска, предупреждение
/// о длине сессии). Tauri-agnostic — драйвится напрямую интеграционным тестом.
pub fn run_consumer(
    consumer: ring::Consumer,
    cfg: ConsumerConfig,
    level_cb: LevelCallback,
    stop: Arc<AtomicBool>,
    rel: ConsumerReliability,
) -> Result<Vec<SegmentInfo>, AudioError> {
    let mut writer = SegmentWriter::new(SegmentConfig {
        dir: cfg.output_dir.clone(),
        sample_rate_hz: cfg.sample_rate_hz,
        channels: cfg.target_channels,
        bits_per_sample: cfg.bit_depth,
        segment_seconds: cfg.segment_seconds,
        flush_interval: cfg.flush_interval,
    })?;

    let mut scratch = vec![0.0f32; cfg.scratch_len.max(1)];
    let poll = Duration::from_secs_f64(1.0 / (cfg.level_update_hz.max(1) as f64));
    let mut last_emit = Instant::now();
    let session_start = Instant::now();
    let mut max_warned = false;
    let mut last_disk_status = DiskStatus::Ok;

    'run: loop {
        let n = consumer.pop_slice(&mut scratch);
        if n > 0 {
            let mono = convert::downmix(&scratch[..n], cfg.native_channels, cfg.target_channels);
            let pcm = convert::quantize_i16(&mono);
            writer.write_samples(&pcm)?;
            if last_emit.elapsed() >= poll {
                level_cb(level_of(&mono));
                last_emit = Instant::now();
            }
            writer.maybe_flush()?;

            // По факту закрытия сегмента: журнал + зеркало + контроль диска.
            for seg in writer.drain_completed() {
                rel.journal(&JournalRecord::SegmentCompleted {
                    index: seg.index,
                    path: seg.path.to_string_lossy().into_owned(),
                    frames: seg.frames,
                });
                if let Some(mirror) = &rel.mirror {
                    if let Err(e) = mirror.mirror_segment(&seg) {
                        eprintln!("[reliability] зеркалирование {:?} не удалось: {e}", seg.path);
                    }
                }
                // Контроль диска с шагом «сегмент» (config-derived cadence).
                if check_disk(&rel, &mut last_disk_status) == DiskStatus::Critical {
                    break 'run; // защитный стоп: ниже finalize гарантирует флаш
                }
            }

            // Предупреждение о длине сессии (v1: только событие).
            if let Some(maxd) = rel.max_session {
                if !max_warned && session_start.elapsed() >= maxd {
                    rel.journal(&JournalRecord::MaxDurationWarning);
                    rel.emit(ReliabilityEvent::MaxDurationWarning);
                    max_warned = true;
                }
            }
            continue;
        }

        // Буфер пуст: при остановке — завершаем (остаток уже слит выше).
        if stop.load(Ordering::Acquire) {
            break;
        }
        if last_emit.elapsed() >= poll {
            level_cb(LevelEvent {
                peak: 0.0,
                rms: 0.0,
            });
            last_emit = Instant::now();
        }
        writer.maybe_flush()?;
        thread::sleep(poll);
    }

    writer.finalize()
}

/// Замерить свободное место и при ухудшении статуса — оповестить/журналировать.
/// Возвращает текущий статус (`Critical` сигналит вызывающему о защитном стопе).
fn check_disk(rel: &ConsumerReliability, last: &mut DiskStatus) -> DiskStatus {
    let Some(disk) = &rel.disk else {
        return DiskStatus::Ok;
    };
    // Сбой замера не должен ронять запись: трактуем как «достаточно места».
    let Ok(free_mb) = disk_monitor::free_space_mb(&disk.path) else {
        return DiskStatus::Ok;
    };
    let status = disk_monitor::classify(free_mb, disk.thresholds);
    if status != *last {
        match status {
            DiskStatus::Low => {
                rel.journal(&JournalRecord::DiskLow { free_mb });
                rel.emit(ReliabilityEvent::DiskLow { free_mb });
            }
            DiskStatus::Critical => {
                rel.journal(&JournalRecord::DiskCritical { free_mb });
                rel.emit(ReliabilityEvent::DiskCritical { free_mb });
            }
            DiskStatus::Ok => {}
        }
        *last = status;
    }
    status
}

/// Пик (макс. модуль) и RMS нормированного сигнала.
fn level_of(samples: &[f32]) -> LevelEvent {
    if samples.is_empty() {
        return LevelEvent {
            peak: 0.0,
            rms: 0.0,
        };
    }
    let mut peak = 0.0f32;
    let mut sumsq = 0.0f64;
    for &s in samples {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
        sumsq += (s as f64) * (s as f64);
    }
    let rms = (sumsq / samples.len() as f64).sqrt() as f32;
    LevelEvent { peak, rms }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_of_empty_is_zero() {
        let l = level_of(&[]);
        assert_eq!(l.peak, 0.0);
        assert_eq!(l.rms, 0.0);
    }

    #[test]
    fn level_of_constant_signal() {
        let l = level_of(&[0.5, -0.5, 0.5, -0.5]);
        assert!((l.peak - 0.5).abs() < 1e-6);
        assert!((l.rms - 0.5).abs() < 1e-6);
    }
}
