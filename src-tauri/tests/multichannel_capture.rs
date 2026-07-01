//! Интеграционный тест многоканального захвата (этап 09 —
//! `promts/09_multichannel.md`, «Тесты»). Драйвит per-track consumer-конвейеры
//! синтетическим многоканальным сигналом через отдельные кольцевые буферы (без
//! реального `cpal` — портируемо в CI, как `capture_pipeline.rs`).
//!
//! Проверяет: (1) каждая дорожка извлекает **свой** канал (семпл-точно);
//! (2) дорожки семпл-выровнены (равные длины сегментов при равном входе);
//! (3) обрыв/пустой поток одной дорожки не влияет на сегменты другой.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use court_audio_lib::audio::capture::{
    run_consumer, ConsumerConfig, ConsumerReliability, LevelEvent,
};
use court_audio_lib::audio::ring;

fn expect_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

fn sine(frames: usize, rate: u32, freq: f32, amp: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * amp)
        .collect()
}

/// Конфиг дорожки: извлекаем канал `channel_index` из `native_channels`-потока.
fn track_cfg(
    dir: std::path::PathBuf,
    rate: u32,
    native_channels: u16,
    channel_index: u16,
    track_id: u32,
    scratch_len: usize,
) -> ConsumerConfig {
    ConsumerConfig {
        native_channels,
        target_channels: 1, // дорожка моно
        bit_depth: 16,
        sample_rate_hz: rate,
        segment_seconds: 1,
        flush_interval: Duration::from_millis(1_500),
        level_update_hz: 25,
        output_dir: dir,
        scratch_len,
        channel_index: Some(channel_index),
        track_id,
    }
}

/// Прогнать дорожку до конца по заранее наполненному кольцевому буферу.
fn drain_track(cfg: ConsumerConfig, interleaved: &[f32]) -> Vec<std::path::PathBuf> {
    let (producer, consumer) = ring::channel(interleaved.len() + 16);
    assert_eq!(producer.push_slice(interleaved), interleaved.len());
    let stop = Arc::new(AtomicBool::new(true));
    let level_cb = Box::new(|_: LevelEvent| {});
    let scratch = consumer.capacity();
    let cfg = ConsumerConfig { scratch_len: scratch, ..cfg };
    let segs = run_consumer(consumer, cfg, level_cb, stop, ConsumerReliability::none()).unwrap();
    segs.into_iter().map(|s| s.path).collect()
}

#[test]
fn each_track_extracts_its_own_channel_sample_accurately() {
    let tmp = tempfile::tempdir().unwrap();
    let rate = 8_000u32;
    let frames = 12_000usize; // 1.5 c при segment_seconds=1 → 2 сегмента (8000+4000)

    // Двухканальный интерлив: канал 0 — судья (440 Гц), канал 1 — защита (330 Гц).
    let judge = sine(frames, rate, 440.0, 0.7);
    let defense = sine(frames, rate, 330.0, 0.4);
    let mut interleaved = Vec::with_capacity(frames * 2);
    for (&j, &d) in judge.iter().zip(defense.iter()) {
        interleaved.push(j);
        interleaved.push(d);
    }

    let judge_dir = tmp.path().join("track-0-judge");
    let defense_dir = tmp.path().join("track-1-defense");
    std::fs::create_dir_all(&judge_dir).unwrap();
    std::fs::create_dir_all(&defense_dir).unwrap();

    let judge_segs = drain_track(track_cfg(judge_dir, rate, 2, 0, 0, 0), &interleaved);
    let defense_segs = drain_track(track_cfg(defense_dir, rate, 2, 1, 1, 0), &interleaved);

    // (2) Семпл-выравнивание: у обеих дорожек одинаковое число сегментов и длины.
    assert_eq!(judge_segs.len(), 2);
    assert_eq!(defense_segs.len(), 2);
    let jlen: Vec<u32> = judge_segs
        .iter()
        .map(|p| hound::WavReader::open(p).unwrap().len())
        .collect();
    let dlen: Vec<u32> = defense_segs
        .iter()
        .map(|p| hound::WavReader::open(p).unwrap().len())
        .collect();
    assert_eq!(jlen, vec![8_000, 4_000]);
    assert_eq!(dlen, vec![8_000, 4_000]);

    // (1) Каждая дорожка несёт данные именно своего канала (семпл-точно).
    let jgot: Vec<i16> = hound::WavReader::open(&judge_segs[0])
        .unwrap()
        .samples::<i16>()
        .take(16)
        .map(|r| r.unwrap())
        .collect();
    let jwant: Vec<i16> = judge[..16].iter().map(|&s| expect_i16(s)).collect();
    assert_eq!(jgot, jwant, "дорожка судьи = канал 0");

    let dgot: Vec<i16> = hound::WavReader::open(&defense_segs[0])
        .unwrap()
        .samples::<i16>()
        .take(16)
        .map(|r| r.unwrap())
        .collect();
    let dwant: Vec<i16> = defense[..16].iter().map(|&s| expect_i16(s)).collect();
    assert_eq!(dgot, dwant, "дорожка защиты = канал 1");
}

#[test]
fn one_track_silent_does_not_affect_the_other() {
    // (3) Изоляция: у дорожки 1 «оборвалось устройство» — её поток пуст. Дорожка
    // 0 всё равно пишет полноценные сегменты (обрыв одного канала не роняет
    // остальные — per-track consumer'ы независимы).
    let tmp = tempfile::tempdir().unwrap();
    let rate = 8_000u32;
    let frames = 8_000usize; // ровно 1 сегмент

    let judge = sine(frames, rate, 440.0, 0.6);
    // Двухканальный поток для судьи; защита будет получать ПУСТОЙ поток.
    let mut interleaved = Vec::with_capacity(frames * 2);
    for &j in judge.iter() {
        interleaved.push(j);
        interleaved.push(0.0);
    }

    let judge_dir = tmp.path().join("track-0-judge");
    let defense_dir = tmp.path().join("track-1-defense");
    std::fs::create_dir_all(&judge_dir).unwrap();
    std::fs::create_dir_all(&defense_dir).unwrap();

    let judge_segs = drain_track(track_cfg(judge_dir, rate, 2, 0, 0, 0), &interleaved);
    // Защита: пустой вход (обрыв) — ни одного сегмента, но и без паники.
    let defense_segs = drain_track(track_cfg(defense_dir, rate, 2, 1, 1, 0), &[]);

    assert_eq!(judge_segs.len(), 1, "судья пишет несмотря на обрыв защиты");
    assert_eq!(
        hound::WavReader::open(&judge_segs[0]).unwrap().len(),
        frames as u32
    );
    assert!(defense_segs.is_empty(), "у оборванной дорожки нет сегментов");
}
