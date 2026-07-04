//! Интеграционный тест превью-мониторинга уровня (этап 13.2 —
//! `promts/13_2_multichannel_monitor.md`, «Тесты»). Драйвит Tauri-agnostic
//! `run_monitor` синтетическим сигналом через кольцевой буфер (без реального
//! `cpal` — портируемо в CI, как `capture_pipeline.rs`).
//!
//! Проверяет: (1) многоканал — каждая дорожка эмитит уровни под своим `track_id`,
//! извлекая свой канал (включая «дубль» одного канала на двух дорожках);
//! (2) одноканал — один поток, `track_id 0`, уровни по нативным каналам;
//! (3) поток монитора чисто останавливается по флагу (нет утечки потоков).
//!
//! Реальный микрофон/устройство — ручная приёмка (CI не имеет устройства ввода).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use court_audio_lib::audio::capture::{run_monitor, LevelEvent, MonitorConfig};
use court_audio_lib::audio::ring;

/// Прогнать `run_monitor` на **непрерывном** синтетическом потоке (как реальный
/// `cpal`-источник): producer-поток постоянно доливает `interleaved` в кольцевой
/// буфер, monitor-поток считает уровни. Возвращаем события уровня до первого,
/// отражающего сигнал; затем корректно останавливаем оба потока (проверка
/// чистой остановки — без утечки). Непрерывность важна: троттлинг эмиссии
/// пропускает первый отсчёт, а разовый буфер к тому моменту уже осушён.
fn drive_monitor(cfg: MonitorConfig, interleaved: &[f32]) -> Vec<LevelEvent> {
    // Ёмкость с запасом на несколько «кадров» подачи (producer доливает по кругу).
    let (producer, consumer) = ring::channel(interleaved.len() * 8 + 16);

    let stop = Arc::new(AtomicBool::new(false));
    let stop_producer = Arc::clone(&stop);
    let stop_monitor = Arc::clone(&stop);

    let feed = interleaved.to_vec();
    let producer_handle = std::thread::Builder::new()
        .name("monitor-feed".into())
        .spawn(move || {
            while !stop_producer.load(Ordering::Acquire) {
                producer.push_slice(&feed);
                std::thread::sleep(Duration::from_millis(1));
            }
        })
        .unwrap();

    let (tx, rx) = mpsc::channel::<LevelEvent>();
    let level_cb = Box::new(move |lv: LevelEvent| {
        let _ = tx.send(lv);
    });
    let monitor_handle = std::thread::Builder::new()
        .name("monitor-test".into())
        .spawn(move || run_monitor(consumer, cfg, level_cb, stop_monitor))
        .unwrap();

    // Ждём первое событие, отражающее поданный сигнал (непустые каналы). Таймаут
    // страхует CI от бесконечного ожидания при регрессии эмиссии.
    let mut events = Vec::new();
    while let Ok(ev) = rx.recv_timeout(Duration::from_secs(5)) {
        let has_signal = ev.channels.iter().any(|c| c.peak > 0.0);
        events.push(ev);
        if has_signal {
            break;
        }
    }

    stop.store(true, Ordering::Release);
    monitor_handle.join().unwrap(); // поток завершился по флагу — нет утечки
    producer_handle.join().unwrap();
    // Убеждаемся, что дождались именно сигнала (иначе тест-регрессия эмиссии).
    assert!(
        events.iter().any(|e| e.channels.iter().any(|c| c.peak > 0.0)),
        "монитор не эмитил уровни поданного сигнала"
    );
    events
}

#[test]
fn multichannel_monitor_emits_under_each_track_id() {
    // Двухканальное устройство: канал 0 громкий (±0.8), канал 1 тихий (±0.2).
    let interleaved = [0.8, 0.2, -0.8, -0.2, 0.8, 0.2, -0.8, -0.2];

    // Дорожка 0 → канал 0 под track_id 0.
    let t0 = drive_monitor(
        MonitorConfig {
            native_channels: 2,
            level_update_hz: 1_000, // частый опрос: тест не ждёт
            channel_index: Some(0),
            track_id: 0,
        },
        &interleaved,
    );
    let sig0 = t0.iter().rev().find(|e| e.channels[0].peak > 0.0).unwrap();
    assert_eq!(sig0.track_id, 0);
    assert_eq!(sig0.channels.len(), 1, "дорожка моно");
    assert!((sig0.channels[0].peak - 0.8).abs() < 1e-6, "именно канал 0");

    // Дорожка 1 → канал 1 под track_id 1.
    let t1 = drive_monitor(
        MonitorConfig {
            native_channels: 2,
            level_update_hz: 1_000,
            channel_index: Some(1),
            track_id: 1,
        },
        &interleaved,
    );
    let sig1 = t1.iter().rev().find(|e| e.channels[0].peak > 0.0).unwrap();
    assert_eq!(sig1.track_id, 1);
    assert!((sig1.channels[0].peak - 0.2).abs() < 1e-6, "именно канал 1");
}

#[test]
fn duplicate_channel_on_two_tracks_both_live() {
    // «Дубль»: две дорожки читают канал 0 того же устройства, но эмитят под
    // своими track_id — до записи оба индикатора живые (совпадает с записью).
    let interleaved = [0.5, -0.1, -0.5, 0.1, 0.5, -0.1, -0.5, 0.1];

    let a = drive_monitor(
        MonitorConfig {
            native_channels: 2,
            level_update_hz: 1_000,
            channel_index: Some(0),
            track_id: 2,
        },
        &interleaved,
    );
    let b = drive_monitor(
        MonitorConfig {
            native_channels: 2,
            level_update_hz: 1_000,
            channel_index: Some(0),
            track_id: 5,
        },
        &interleaved,
    );

    let siga = a.iter().rev().find(|e| e.channels[0].peak > 0.0).unwrap();
    let sigb = b.iter().rev().find(|e| e.channels[0].peak > 0.0).unwrap();
    assert_eq!(siga.track_id, 2);
    assert_eq!(sigb.track_id, 5);
    // Оба несут данные канала 0 (один источник, разные дорожки).
    assert!((siga.channels[0].peak - 0.5).abs() < 1e-6);
    assert!((sigb.channels[0].peak - 0.5).abs() < 1e-6);
}

#[test]
fn single_channel_monitor_keeps_native_channels_track0() {
    // Одноканал (channel_index None): уровни по всем нативным каналам, track_id 0.
    let interleaved = [0.7, 0.3, -0.7, -0.3, 0.7, 0.3];
    let events = drive_monitor(
        MonitorConfig {
            native_channels: 2,
            level_update_hz: 1_000,
            channel_index: None,
            track_id: 0,
        },
        &interleaved,
    );
    let sig = events
        .iter()
        .rev()
        .find(|e| e.channels.iter().any(|c| c.peak > 0.0))
        .unwrap();
    assert_eq!(sig.track_id, 0);
    assert_eq!(sig.channels.len(), 2, "оба нативных канала");
    assert!((sig.channels[0].peak - 0.7).abs() < 1e-6);
    assert!((sig.channels[1].peak - 0.3).abs() < 1e-6);
}
