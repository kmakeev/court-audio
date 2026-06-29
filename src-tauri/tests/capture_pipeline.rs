//! Интеграционный тест конвейера захвата (этап 01 — `promts/01_audio_core.md`,
//! «Тесты»). Драйвит Tauri-agnostic consumer-конвейер синтетическим сигналом
//! через кольцевой буфер (без реального `cpal`-устройства — портируемо в CI) и
//! проверяет, что сегменты валидны и **непрерывны по времени** (без пропусков и
//! наложений на стыках, суммарная длительность сохранена).
//!
//! Реальный микрофон/loopback — ручная приёмка (CI не имеет устройства ввода).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use court_audio_lib::audio::capture::{run_consumer, ConsumerConfig, ConsumerReliability, LevelEvent};
use court_audio_lib::audio::ring;

/// Эталонное квантование (зеркало `convert::quantize_i16`) для сверки данных.
fn expect_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

fn sine(frames: usize, rate: u32, freq: f32, amp: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * amp)
        .collect()
}

#[test]
fn mono_capture_produces_valid_continuous_segments() {
    let tmp = tempfile::tempdir().unwrap();
    let rate = 8_000u32;
    let total_frames = 20_000usize; // 2.5 c при rate=8000 -> 3 сегмента по 1 c
    let signal = sine(total_frames, rate, 440.0, 0.5);

    // Ёмкость с запасом, чтобы один pop забрал весь сигнал.
    let (producer, consumer) = ring::channel(total_frames + 16);
    assert_eq!(producer.push_slice(&signal), total_frames);
    assert_eq!(producer.dropped(), 0);

    let cfg = ConsumerConfig {
        native_channels: 1,
        target_channels: 1,
        bit_depth: 16,
        sample_rate_hz: rate,
        segment_seconds: 1, // порог ротации = 8000 кадров
        flush_interval: Duration::from_millis(1_500),
        level_update_hz: 25,
        output_dir: tmp.path().to_path_buf(),
        scratch_len: consumer.capacity(),
    };

    // Останавливаем заранее: consumer сольёт буфер и завершится.
    let stop = Arc::new(AtomicBool::new(true));
    let level_cb = Box::new(|_: LevelEvent| {});
    let segments = run_consumer(consumer, cfg, level_cb, stop, ConsumerReliability::none()).unwrap();

    // Три сегмента: 8000 + 8000 + 4000 кадров, без потерь/наложений.
    assert_eq!(segments.len(), 3);
    let lens: Vec<u32> = segments
        .iter()
        .map(|s| {
            let reader = hound::WavReader::open(&s.path).unwrap();
            let spec = reader.spec();
            assert_eq!(spec.channels, 1);
            assert_eq!(spec.sample_rate, rate);
            assert_eq!(spec.bits_per_sample, 16);
            reader.len()
        })
        .collect();
    assert_eq!(lens, vec![8_000, 8_000, 4_000]);
    assert_eq!(lens.iter().sum::<u32>() as usize, total_frames);

    // Индексы последовательны (непрерывность нумерации сегментов).
    let idx: Vec<u32> = segments.iter().map(|s| s.index).collect();
    assert_eq!(idx, vec![1, 2, 3]);

    // Данные первого сегмента соответствуют квантованному сигналу (целостность).
    let mut reader = hound::WavReader::open(&segments[0].path).unwrap();
    let got: Vec<i16> = reader
        .samples::<i16>()
        .take(16)
        .map(|r| r.unwrap())
        .collect();
    let want: Vec<i16> = signal[..16].iter().map(|&s| expect_i16(s)).collect();
    assert_eq!(got, want);
}

#[test]
fn stereo_input_is_downmixed_to_mono() {
    let tmp = tempfile::tempdir().unwrap();
    let rate = 8_000u32;
    let frames = 12_000usize; // 1.5 c -> 2 сегмента (8000 + 4000)

    // Интерливнутое стерео: L = sine, R = 0 -> моно = sine/2.
    let mut interleaved = Vec::with_capacity(frames * 2);
    let left = sine(frames, rate, 330.0, 0.8);
    for &l in &left {
        interleaved.push(l);
        interleaved.push(0.0);
    }

    let (producer, consumer) = ring::channel(interleaved.len() + 16);
    assert_eq!(producer.push_slice(&interleaved), interleaved.len());

    let cfg = ConsumerConfig {
        native_channels: 2,
        target_channels: 1,
        bit_depth: 16,
        sample_rate_hz: rate,
        segment_seconds: 1,
        flush_interval: Duration::from_millis(1_500),
        level_update_hz: 25,
        output_dir: tmp.path().to_path_buf(),
        scratch_len: consumer.capacity(),
    };

    let stop = Arc::new(AtomicBool::new(true));
    let level_cb = Box::new(|_: LevelEvent| {});
    let segments = run_consumer(consumer, cfg, level_cb, stop, ConsumerReliability::none()).unwrap();

    let total: u32 = segments
        .iter()
        .map(|s| hound::WavReader::open(&s.path).unwrap().len())
        .sum();
    // Каждый кадр стерео -> один моно-семпл; непрерывность сохранена.
    assert_eq!(total as usize, frames);
    assert_eq!(segments.len(), 2);

    // Downmix корректен: L + R = L + 0, среднее = L/2.
    let mut reader = hound::WavReader::open(&segments[0].path).unwrap();
    let got: Vec<i16> = reader
        .samples::<i16>()
        .take(8)
        .map(|r| r.unwrap())
        .collect();
    let want: Vec<i16> = left[..8].iter().map(|&l| expect_i16(l * 0.5)).collect();
    assert_eq!(got, want);
}
