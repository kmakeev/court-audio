//! Интеграционные симуляции надёжности (этап 02 —
//! `promts/02_recorder_reliability.md`, «Тесты»/«Критерии приёмки»). Все —
//! без реального устройства (портируемо в CI):
//!
//! - **нехватка места**: критический порог → корректный стоп конвейера с
//!   гарантированным флашем, данные целы, событие/журнал зафиксированы;
//! - **зеркало**: завершённые сегменты копируются на второй носитель;
//! - **обрыв устройства**: пробник false→true → пауза → авто-возобновление;
//! - **watchdog**: «застывший» heartbeat → срабатывание.

use std::cell::Cell;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use court_audio_lib::audio::capture::{
    run_consumer, ConsumerConfig, ConsumerReliability, DiskWatch, LevelEvent, ReliabilityEvent,
};
use court_audio_lib::audio::ring;
use court_audio_lib::recorder::journal::{self, Journal, JournalRecord};
use court_audio_lib::recorder::session::{SessionEvent, SessionMachine, SessionState};
use court_audio_lib::reliability::device_monitor::{
    apply_device_event, DeviceEvent, DeviceMonitor,
};
use court_audio_lib::reliability::disk_monitor::DiskThresholds;
use court_audio_lib::reliability::watchdog::{is_stalled, now_unix_ms, Heartbeat};

fn ramp(frames: usize) -> Vec<f32> {
    (0..frames).map(|i| (i % 50) as f32 / 100.0).collect()
}

fn base_cfg(dir: std::path::PathBuf, rate: u32, scratch_len: usize) -> ConsumerConfig {
    ConsumerConfig {
        native_channels: 1,
        target_channels: 1,
        bit_depth: 16,
        sample_rate_hz: rate,
        segment_seconds: 1,
        flush_interval: Duration::from_millis(1_500),
        level_update_hz: 25,
        output_dir: dir,
        scratch_len,
        channel_index: None,
        track_id: 0,
    }
}

#[test]
fn disk_critical_triggers_clean_stop_without_data_loss() {
    let tmp = tempfile::tempdir().unwrap();
    let session = tmp.path().join("session-1");

    let rate = 8_000u32;
    let total_frames = 20_000usize; // 2.5 сегмента
    let signal = ramp(total_frames);
    let (producer, consumer) = ring::channel(total_frames + 16);
    producer.push_slice(&signal);

    let jrnl = Journal::open(&session).unwrap();
    let jrnl = Arc::new(Mutex::new(jrnl));

    let events: Arc<Mutex<Vec<ReliabilityEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let events_cb = Arc::clone(&events);
    let on_event: court_audio_lib::audio::capture::ReliabilityCallback =
        Arc::new(move |ev: ReliabilityEvent| events_cb.lock().unwrap().push(ev));

    let rel = ConsumerReliability {
        journal: Some(Arc::clone(&jrnl)),
        mirror: None,
        // Пороги заведомо выше любого реального свободного места -> Critical
        // срабатывает детерминированно на первом же завершённом сегменте.
        disk: Some(DiskWatch {
            path: session.clone(),
            thresholds: DiskThresholds {
                low_mb: u64::MAX,
                critical_mb: u64::MAX,
            },
        }),
        max_session: None,
        on_event: Some(on_event),
    };

    // stop=false: остановка должна прийти именно от критического порога диска.
    let stop = Arc::new(AtomicBool::new(false));
    let segments = run_consumer(
        consumer,
        base_cfg(session.clone(), rate, total_frames + 16),
        Box::new(|_: LevelEvent| {}),
        stop,
        rel,
    )
    .unwrap();

    // Данные целы: всё записанное до стопа финализировано, сумма кадров сохранена.
    assert!(!segments.is_empty());
    let total: u64 = segments
        .iter()
        .map(|s| hound::WavReader::open(&s.path).unwrap().len() as u64)
        .sum();
    assert_eq!(total, total_frames as u64);

    // Событие и журнал зафиксировали критический порог.
    let evs = events.lock().unwrap();
    assert!(evs
        .iter()
        .any(|e| matches!(e, ReliabilityEvent::DiskCritical { .. })));
    drop(evs);
    let replayed = journal::replay(&session.join(journal::JOURNAL_FILE_NAME)).unwrap();
    // Проверяем наличие критической записи через повторное чтение журнала.
    let raw = std::fs::read_to_string(session.join(journal::JOURNAL_FILE_NAME)).unwrap();
    assert!(raw.contains("disk_critical"));
    assert!(!replayed.completed_segments.is_empty());
}

#[test]
fn segments_are_mirrored_to_second_storage() {
    let tmp = tempfile::tempdir().unwrap();
    // Структура зеркала = структуре основного места (этап 13.3): корень хранилища
    // — база, сессия — подкаталог под ним, зеркало повторяет дерево.
    let storage_root = tmp.path().join("recordings");
    let session = storage_root.join("session-1");
    let mirror_root = tmp.path().join("mirror");

    let rate = 8_000u32;
    let total_frames = 16_000usize; // 2 полных сегмента
    let signal = ramp(total_frames);
    let (producer, consumer) = ring::channel(total_frames + 16);
    producer.push_slice(&signal);

    let rel = ConsumerReliability {
        journal: None,
        mirror: Some(
            court_audio_lib::reliability::mirror::Mirror::new(&storage_root, &mirror_root).unwrap(),
        ),
        disk: None,
        max_session: None,
        on_event: None,
    };

    let stop = Arc::new(AtomicBool::new(true));
    let segments = run_consumer(
        consumer,
        base_cfg(session.clone(), rate, total_frames + 16),
        Box::new(|_: LevelEvent| {}),
        stop,
        rel,
    )
    .unwrap();

    // Зеркало повторяет дерево: <mirror>/session-1/seg-*.wav (не плоский каталог).
    let mirror_session = mirror_root.join("session-1");
    let mirrored: Vec<_> = std::fs::read_dir(&mirror_session)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(!mirrored.is_empty());
    // Каждый зеркальный файл побайтно равен оригиналу в подкаталоге сессии.
    for entry in &mirrored {
        let name = entry.file_name();
        let original = session.join(&name);
        assert!(original.exists());
        assert_eq!(
            std::fs::read(&original).unwrap(),
            std::fs::read(entry.path()).unwrap()
        );
    }
    // Хотя бы один завершённый сегмент был сделан.
    assert!(segments.len() >= 2);
}

#[test]
fn device_loss_pauses_and_returns_resumes() {
    // Сценарий обрыва: пробник видит устройство, затем теряет, затем возвращает.
    let present = Cell::new(true);
    let mut monitor = DeviceMonitor::new(|| present.get());
    let mut machine = SessionMachine::new();
    machine.apply(SessionEvent::Start).unwrap();

    // Обрыв -> пауза с пометкой device_lost.
    present.set(false);
    let event = monitor.poll().unwrap();
    assert_eq!(event, DeviceEvent::Lost);
    let state = apply_device_event(&mut machine, event, /* auto_resume = */ true);
    assert_eq!(state, Some(SessionState::Paused));

    // Возврат -> авто-возобновление.
    present.set(true);
    let event = monitor.poll().unwrap();
    assert_eq!(event, DeviceEvent::Back);
    let state = apply_device_event(&mut machine, event, true);
    assert_eq!(state, Some(SessionState::Recording));
}

#[test]
fn watchdog_detects_stall() {
    let hb = Heartbeat::new();
    // Свежий heartbeat — «жив».
    assert!(!is_stalled(now_unix_ms(), hb.last(), 5_000));
    // Отметка далеко в прошлом — «завис».
    hb.beat_at(now_unix_ms().saturating_sub(10_000));
    assert!(is_stalled(now_unix_ms(), hb.last(), 5_000));
}

#[test]
fn journal_records_lifecycle_for_recovery() {
    // Демонстрация журналирования lifecycle (как делает IPC) для восстановления.
    let tmp = tempfile::tempdir().unwrap();
    let mut j = Journal::open(tmp.path()).unwrap();
    j.append(&JournalRecord::SessionStarted {
        started_at_unix_ms: 1,
        sample_rate_hz: 8_000,
        channels: 1,
        bit_depth: 16,
        segment_seconds: 1,
        operator_id: String::new(),
        station_id: String::new(),
    })
    .unwrap();
    j.append(&JournalRecord::Stopped).unwrap();
    let state = journal::replay(&tmp.path().join(journal::JOURNAL_FILE_NAME)).unwrap();
    assert!(!state.is_unfinished());
}
