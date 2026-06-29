//! Интеграционный «kill-тест» восстановления (этап 02 —
//! `promts/02_recorder_reliability.md`, «Критерии приёмки»).
//!
//! Симуляция внезапного завершения процесса в ходе записи (CI без устройства):
//! конвейер пишет несколько сегментов + журнал; затем «обрываем» сессию —
//! портим заголовок и усекаем последний сегмент, **не** записываем `Stopped`.
//! После «рестарта»: [`recovery::scan_unfinished`] находит сессию,
//! [`recovery::recover_in_place`] чинит последний сегмент → все сегменты
//! валидны (читаются hound'ом), потеря ≤ flush-интервала, нумерация цела.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use court_audio_lib::audio::capture::{run_consumer, ConsumerConfig, ConsumerReliability, LevelEvent};
use court_audio_lib::audio::ring;
use court_audio_lib::recorder::journal::{self, Journal, JournalRecord};
use court_audio_lib::recorder::recovery::{self, RepairOutcome};

fn ramp(frames: usize) -> Vec<f32> {
    (0..frames).map(|i| (i % 50) as f32 / 100.0).collect()
}

#[test]
fn killtest_recovers_unfinished_session() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let session = root.join("session-1");

    let rate = 8_000u32;
    // 2.5 сегмента по 1 c -> seg1, seg2 (закрыты ротацией), seg3 (частичный).
    let total_frames = 20_000usize;
    let signal = ramp(total_frames);

    let (producer, consumer) = ring::channel(total_frames + 16);
    assert_eq!(producer.push_slice(&signal), total_frames);

    // Журнал сессии: SessionStarted + (через consumer) SegmentCompleted для
    // закрытых ротацией сегментов. `Stopped` намеренно НЕ пишем (имитация краха).
    let jrnl = Journal::open(&session).unwrap();
    let jrnl = Arc::new(Mutex::new(jrnl));
    jrnl.lock()
        .unwrap()
        .append(&JournalRecord::SessionStarted {
            started_at_unix_ms: 1,
            sample_rate_hz: rate,
            channels: 1,
            bit_depth: 16,
            segment_seconds: 1,
        })
        .unwrap();

    let cfg = ConsumerConfig {
        native_channels: 1,
        target_channels: 1,
        bit_depth: 16,
        sample_rate_hz: rate,
        segment_seconds: 1, // порог ротации = 8000 кадров
        flush_interval: Duration::from_millis(1_500),
        level_update_hz: 25,
        output_dir: session.clone(),
        scratch_len: consumer.capacity(),
    };

    let rel = ConsumerReliability {
        journal: Some(Arc::clone(&jrnl)),
        mirror: None,
        disk: None,
        max_session: None,
        on_event: None,
    };

    let stop = Arc::new(AtomicBool::new(true));
    let segments = run_consumer(
        consumer,
        cfg,
        Box::new(|_: LevelEvent| {}),
        stop,
        rel,
    )
    .unwrap();
    assert_eq!(segments.len(), 3, "ожидаем seg1, seg2 и частичный seg3");

    // Журнал зафиксировал завершение двух сегментов, закрытых ротацией.
    let replay_before = journal::replay(&session.join(journal::JOURNAL_FILE_NAME)).unwrap();
    assert_eq!(replay_before.completed_segments.len(), 2);
    assert!(replay_before.is_unfinished());

    // ── Имитация краха питания в третьем сегменте ────────────────────────────
    // Портим размеры в заголовке и усекаем хвост (несколько кадров < flush).
    let files = recovery::segment_files(&session).unwrap();
    assert_eq!(files.len(), 3);
    let last = files.last().unwrap();
    let valid_frames_before = hound::WavReader::open(last).unwrap().len();

    let mut bytes = std::fs::read(last).unwrap();
    // Усекаем 10 кадров (10 байт×2 = 20 байт) — заведомо меньше flush-интервала.
    bytes.truncate(bytes.len() - 20);
    // Обнуляем размеры RIFF/data (как при сбросе до flush).
    bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
    // Найдём data-чанк простым поиском id (заголовок hound каноничен).
    let data_pos = find_data_chunk(&bytes).expect("data-чанк");
    bytes[data_pos + 4..data_pos + 8].copy_from_slice(&0u32.to_le_bytes());
    std::fs::write(last, &bytes).unwrap();

    // hound теперь видит «нулевую» длину — сегмент сломан.
    assert_eq!(hound::WavReader::open(last).unwrap().len(), 0);

    // ── «Рестарт»: обнаружение и починка ─────────────────────────────────────
    let unfinished = recovery::scan_unfinished(root).unwrap();
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].dir, session);

    let outcome = recovery::recover_in_place(&session).unwrap();
    assert!(matches!(outcome, Some(RepairOutcome::Repaired { .. })));

    // Все сегменты валидны; первые два — без потерь, третий потерял ≤ усечения.
    let lens: Vec<u32> = files
        .iter()
        .map(|p| hound::WavReader::open(p).unwrap().len())
        .collect();
    assert_eq!(lens[0], 8_000);
    assert_eq!(lens[1], 8_000);
    let recovered_last = lens[2];
    assert!(recovered_last > 0 && recovered_last <= valid_frames_before);
    // Потеря ограничена нашим искусственным усечением (10 кадров).
    assert!(valid_frames_before - recovered_last <= 10);

    // Сессия теперь помечена восстановленной.
    let replay_after = journal::replay(&session.join(journal::JOURNAL_FILE_NAME)).unwrap();
    assert!(replay_after.recovered);
}

/// Найти смещение идентификатора `data`-чанка в каноничном WAV-заголовке.
fn find_data_chunk(bytes: &[u8]) -> Option<usize> {
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        if &bytes[pos..pos + 4] == b"data" {
            return Some(pos);
        }
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        pos += 8 + size + (size & 1);
    }
    None
}
