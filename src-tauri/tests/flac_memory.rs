//! Замер памяти потокового FLAC-экспорта (R-014, этап 13.7 — критерий
//! приёмки B): экспорт ≥1 ч синтетики не должен держать сессию в памяти
//! (пиковая память — O(блок), ориентир < 300 МБ поверх базовой).
//!
//! Тест `#[ignore]`: генерирует ~317 МБ WAV и кодирует его — слишком тяжёл для
//! обычного CI-прогона. Запуск на dev-станции (пример для macOS):
//!
//! ```sh
//! cargo test --release --test flac_memory -- --ignored
//! # с замером пиковой памяти: собрать бинарь и запустить под time -l
//! cargo test --release --test flac_memory --no-run
//! /usr/bin/time -l target/release/deps/flac_memory-<hash> --ignored
//! # (Linux: /usr/bin/time -v …; Windows: Task Manager / gsudo psrecord)
//! ```
//!
//! До R-014 час моно 44.1 кГц требовал ~630 МБ только на `Vec<i32>` + ещё
//! столько же на `ByteSink` (три часа — ~1.9 ГБ, риск OOM на станции с 4 ГБ).

use std::path::Path;

use court_audio_lib::export::flac::encode_wav_to_flac;
use hound::{SampleFormat, WavSpec, WavWriter};

/// Час синтетики: моно, 44.1 кГц, 16 бит — дефолты реестра.
const SAMPLE_RATE_HZ: u32 = 44_100;
const DURATION_SECONDS: u32 = 3_600;

fn write_hour_wav(path: &Path) {
    let spec = WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE_HZ,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut w = WavWriter::create(path, spec).unwrap();
    let frames = SAMPLE_RATE_HZ as u64 * DURATION_SECONDS as u64;
    // Псевдослучайный, но сжимаемый сигнал (пила с медленной огибающей) —
    // ближе к речи, чем тишина/шум, и не даёт FLAC выродиться в крайности.
    for i in 0..frames {
        let saw = (i % 441) as i32 * 64 - 14_112;
        let env = ((i / SAMPLE_RATE_HZ as u64) % 10) as i32 + 1;
        w.write_sample((saw / env) as i16 as i32).unwrap();
    }
    w.finalize().unwrap();
}

#[test]
#[ignore = "тяжёлый замер памяти (≈317 МБ WAV) — запускается вручную при приёмке R-014"]
fn hour_long_flac_export_streams_without_loading_session() {
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("hour.wav");
    let flac = tmp.path().join("hour.flac");
    write_hour_wav(&wav);

    encode_wav_to_flac(&wav, &flac).unwrap();

    // Поток валиден и несёт настоящую длину (декларация для плееров).
    let r = claxon::FlacReader::open(&flac).unwrap();
    assert_eq!(
        r.streaminfo().samples,
        Some(SAMPLE_RATE_HZ as u64 * DURATION_SECONDS as u64)
    );
    // Сжатие действительно произошло (не пустой/вырожденный файл).
    let flac_len = std::fs::metadata(&flac).unwrap().len();
    let wav_len = std::fs::metadata(&wav).unwrap().len();
    assert!(flac_len > 0 && flac_len < wav_len);
}
