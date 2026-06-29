//! Конвейер захвата (этап 01 — `promts/01_audio_core.md`, шаги 4, 6).
//!
//! `cpal` input stream → [`ring`] (producer в аудио-callback'е) → consumer-поток
//! → [`SegmentWriter`]. Аудио-callback **лёгкий**: только приведение sample-
//! формата к `f32` и запись в кольцевой буфер; нормализация (downmix +
//! квантование), расчёт уровней и запись на диск — в consumer-потоке, чтобы не
//! подвешивать realtime-callback.
//!
//! `cpal::Stream` не `Send`, поэтому он живёт на выделенном потоке захвата,
//! управляемом каналом команд (Pause/Resume/Stop). Consumer-поток отделён и
//! тестируется без `cpal` (см. [`run_consumer`] и `tests/capture_pipeline.rs`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, FromSample, Sample, SampleFormat, SampleRate, SizedSample, Stream, StreamConfig,
};
use serde::Serialize;

use super::{convert, ring, AudioError};
use crate::recorder::segment_writer::{SegmentConfig, SegmentInfo, SegmentWriter};

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
    Stop,
}

/// Активная сессия захвата: владеет потоком стрима и consumer-потоком.
pub struct CaptureSession {
    cmd_tx: Sender<ControlCmd>,
    capture_handle: Option<JoinHandle<()>>,
    writer_stop: Arc<AtomicBool>,
    writer_handle: Option<JoinHandle<Result<Vec<SegmentInfo>, AudioError>>>,
    native: NativeFormat,
}

impl CaptureSession {
    /// Запустить захват: подобрать устройство/конфиг, поднять `cpal`-поток и
    /// consumer-поток записи. Блокируется до инициализации потока (или ошибки).
    pub fn start(params: CaptureParams, level_cb: LevelCallback) -> Result<Self, AudioError> {
        let (init_tx, init_rx) = mpsc::channel();
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let thread_params = params.clone();
        let capture_handle = thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || capture_thread(thread_params, init_tx, cmd_rx))
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

        let writer_stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&writer_stop);
        let writer_handle = thread::Builder::new()
            .name("audio-writer".into())
            .spawn(move || run_consumer(consumer, consumer_cfg, level_cb, stop_for_thread))
            .map_err(|e| AudioError::Stream(e.to_string()))?;

        Ok(Self {
            cmd_tx,
            capture_handle: Some(capture_handle),
            writer_stop,
            writer_handle: Some(writer_handle),
            native,
        })
    }

    /// Фактический формат захвата (нативная частота/каналы устройства).
    pub fn native_format(&self) -> NativeFormat {
        self.native
    }

    /// Поставить захват на паузу (устройство остаётся открытым).
    pub fn pause(&self) -> Result<(), AudioError> {
        self.cmd_tx
            .send(ControlCmd::Pause)
            .map_err(|_| AudioError::Stream("поток захвата недоступен".into()))
    }

    /// Возобновить захват после паузы.
    pub fn resume(&self) -> Result<(), AudioError> {
        self.cmd_tx
            .send(ControlCmd::Resume)
            .map_err(|_| AudioError::Stream("поток захвата недоступен".into()))
    }

    /// Остановить захват, дождаться слива остатка буфера и финализации
    /// сегментов; вернуть список записанных сегментов (заглушка манифеста).
    pub fn stop(mut self) -> Result<Vec<SegmentInfo>, AudioError> {
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
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        // Если сессию уронили без stop(), всё равно корректно гасим потоки.
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

/// Поток захвата: владеет `cpal::Stream` (не `Send`), управляется командами.
fn capture_thread(
    params: CaptureParams,
    init_tx: Sender<Result<(ring::Consumer, NativeFormat), AudioError>>,
    cmd_rx: Receiver<ControlCmd>,
) {
    let built = build_stream_and_ring(&params);
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
            ControlCmd::Stop => break,
        }
    }
    drop(stream);
}

/// Подобрать устройство/конфигурацию, поднять кольцевой буфер и `cpal`-поток.
fn build_stream_and_ring(
    params: &CaptureParams,
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
        SampleFormat::I16 => build_input_stream::<i16>(&device, &config, producer),
        SampleFormat::U16 => build_input_stream::<u16>(&device, &config, producer),
        SampleFormat::I32 => build_input_stream::<i32>(&device, &config, producer),
        SampleFormat::F32 => build_input_stream::<f32>(&device, &config, producer),
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

/// Построить типизированный входной поток: callback приводит семплы к `f32` и
/// пишет в кольцевой буфер. `scratch` переиспользуется (без аллокаций после
/// прогрева).
fn build_input_stream<T>(
    device: &Device,
    config: &StreamConfig,
    producer: ring::Producer,
) -> Result<Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut scratch: Vec<f32> = Vec::new();
    let err_fn = |e| eprintln!("[audio] ошибка входного потока cpal: {e}");
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
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
/// уровни (с троттлингом до `level_update_hz`) и пишет сегменты. Tauri-agnostic
/// — драйвится напрямую интеграционным тестом.
pub fn run_consumer(
    consumer: ring::Consumer,
    cfg: ConsumerConfig,
    level_cb: LevelCallback,
    stop: Arc<AtomicBool>,
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

    loop {
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
