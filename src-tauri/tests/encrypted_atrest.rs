//! Интеграционные тесты R-013 (этап 13.7 —
//! `promts/13_7_flac_memory_and_segment_encryption.md`, часть A): шифрование
//! сегментов подключено в live-путь записи. Всё портируемо в CI (без
//! Tauri/устройства): конвейер драйвится напрямую, как в
//! `capture_pipeline`/`recovery_killtest`.
//!
//! Покрытие критериев приёмки:
//! - после стопа в каталоге сессии нет открытых `.wav` — только `.enc`;
//!   журнал/манифест несут фактические stored-имена; хеш-цепочка сходится по
//!   **каноничному** содержимому; экспортная склейка читает `.enc` прозрачно;
//! - зеркало содержит те же `.enc` байт-в-байт + журнал — независимая
//!   реконсиляция по зеркалу проходит;
//! - «питание выдернули»: хвостовой сегмент остаётся открытым WAV, при
//!   восстановлении дожурналивается и дофинализируется в `.enc`, цепочка
//!   сходится;
//! - `encrypt_at_rest = false` (segment_key = None) — plaintext-поведение
//!   без изменений (регресс держат `capture_pipeline`/`recovery_killtest`).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use court_audio_lib::audio::capture::{
    run_consumer, ConsumerConfig, ConsumerReliability, LevelEvent,
};
use court_audio_lib::audio::ring;
use court_audio_lib::export::audio::join_track_to_wav;
use court_audio_lib::integrity::hash;
use court_audio_lib::player::timeline::Timeline;
use court_audio_lib::recorder::journal::{self, Journal, JournalRecord};
use court_audio_lib::recorder::recovery;
use court_audio_lib::reliability::mirror::Mirror;
use court_audio_lib::store::crypto::{self, KeyProvider, PassphraseKeyProvider};
use court_audio_lib::store::manifest::ManifestStore;
use court_audio_lib::store::reconcile;

/// Детерминированный ключ станции без env (как в unit-тестах `store::crypto`).
fn test_key() -> [u8; 32] {
    PassphraseKeyProvider::new("station-secret-13-7", b"0123456789abcdef".to_vec())
        .station_key()
        .unwrap()
}

fn ramp(frames: usize) -> Vec<f32> {
    (0..frames).map(|i| (i % 50) as f32 / 100.0).collect()
}

/// Прогнать конвейер записи с шифрованием: 2.5 сегмента по 1 с при 8 кГц.
/// Возвращает (корень хранилища, каталог сессии, каталог зеркала, кадры).
fn record_encrypted_session(tmp: &Path) -> (PathBuf, PathBuf, PathBuf, usize) {
    let storage_root = tmp.join("recordings");
    let session = storage_root.join("session-1");
    let mirror_root = tmp.join("mirror");
    let rate = 8_000u32;
    let total_frames = 20_000usize; // seg1 (8000), seg2 (8000), хвост seg3 (4000)

    let signal = ramp(total_frames);
    let (producer, consumer) = ring::channel(total_frames + 16);
    assert_eq!(producer.push_slice(&signal), total_frames);

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
            operator_id: String::new(),
            station_id: String::new(),
            autonomous_offline: false,
        })
        .unwrap();

    let cfg = ConsumerConfig {
        native_channels: 1,
        target_channels: 1,
        bit_depth: 16,
        sample_rate_hz: rate,
        segment_seconds: 1,
        flush_interval: Duration::from_millis(1_500),
        level_update_hz: 25,
        output_dir: session.clone(),
        scratch_len: consumer.capacity(),
        channel_index: None,
        track_id: 0,
    };
    let rel = ConsumerReliability {
        journal: Some(Arc::clone(&jrnl)),
        mirror: Some(Mirror::new(&storage_root, &mirror_root).unwrap()),
        disk: None,
        max_session: None,
        on_event: None,
        segment_key: Some(test_key()),
    };

    let stop = Arc::new(AtomicBool::new(true));
    let segments = run_consumer(consumer, cfg, Box::new(|_: LevelEvent| {}), stop, rel).unwrap();
    assert_eq!(segments.len(), 3, "ожидаем seg1, seg2 и частичный seg3");
    // Возвращённые stored-пути — фактические `.enc`.
    for seg in &segments {
        assert!(
            seg.path.to_string_lossy().ends_with(".wav.enc"),
            "stored-путь шифрованный: {:?}",
            seg.path
        );
        assert!(seg.path.exists());
    }
    (storage_root, session, mirror_root, total_frames)
}

/// Файлы сегментов каталога по суффиксу имени.
fn segment_files_with_suffix(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("seg-") && n.ends_with(suffix))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
}

#[test]
fn encrypted_capture_leaves_only_enc_and_roundtrips() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, session, mirror_root, total_frames) = record_encrypted_session(tmp.path());
    let key = test_key();

    // На диске нет ни одного открытого WAV — только `.enc` (критерий A-1).
    assert!(segment_files_with_suffix(&session, ".wav").is_empty());
    let enc_files = segment_files_with_suffix(&session, ".wav.enc");
    assert_eq!(enc_files.len(), 3);

    // Журнал несёт фактические stored-имена (`.enc`).
    let state = journal::replay(&session.join(journal::JOURNAL_FILE_NAME)).unwrap();
    assert_eq!(state.completed_segments.len(), 3);
    for seg in &state.completed_segments {
        assert!(seg.path.ends_with(".wav.enc"), "имя в журнале: {}", seg.path);
    }

    // Штатный стоп + реконсиляция: хеш-цепочка сходится по каноничному
    // (расшифрованному) содержимому.
    Journal::open(&session)
        .unwrap()
        .append(&JournalRecord::Stopped)
        .unwrap();
    let store = ManifestStore::in_memory().unwrap();
    reconcile::reconcile_session(&store, &session, Some(&key))
        .unwrap()
        .unwrap();
    let record = store.get_session("session-1").unwrap().unwrap();
    let segs = store.get_segments("session-1").unwrap();
    assert_eq!(segs.len(), 3);
    let hashes: Vec<String> = segs.iter().map(|s| s.sha256.clone()).collect();
    let links: Vec<String> = segs.iter().map(|s| s.chain_link.clone()).collect();
    assert!(hash::verify_chain(
        &hashes,
        &links,
        record.final_chain_link.as_deref()
    ));
    // Хеш манифеста = хешу каноничного WAV (не шифр-файла).
    let plain0 = crypto::read_segment_plain(Path::new(&segs[0].path), Some(&key)).unwrap();
    assert_eq!(segs[0].sha256, hash::sha256_bytes(&plain0));
    assert_eq!(segs[0].size_bytes, plain0.len() as u64);

    // Экспортная склейка читает `.enc` прозрачно и отдаёт все кадры.
    let tl = Timeline::build(&segs, 0, record.sample_rate_hz);
    let out = tmp.path().join("joined.wav");
    let joined = join_track_to_wav(&tl, Some(&key), &out).unwrap();
    assert_eq!(joined.frames, total_frames as u64);

    // Зеркало содержит те же `.enc` байт-в-байт (критерий A-3)…
    let mirror_session = mirror_root.join("session-1");
    let mirrored = segment_files_with_suffix(&mirror_session, ".wav.enc");
    assert_eq!(mirrored.len(), 3);
    assert!(segment_files_with_suffix(&mirror_session, ".wav").is_empty());
    for (orig, copy) in enc_files.iter().zip(&mirrored) {
        assert_eq!(std::fs::read(orig).unwrap(), std::fs::read(copy).unwrap());
    }
    // …и независимая реконсиляция по зеркалу проходит (журнал доехал «на лету»).
    let mirror_store = ManifestStore::in_memory().unwrap();
    reconcile::reconcile_session(&mirror_store, &mirror_session, Some(&key))
        .unwrap()
        .unwrap();
    let msegs = mirror_store.get_segments("session-1").unwrap();
    assert_eq!(msegs.len(), 3);
    let mhashes: Vec<String> = msegs.iter().map(|s| s.sha256.clone()).collect();
    assert_eq!(mhashes, hashes, "зеркало даёт те же канонические хеши");
}

#[test]
fn crash_recovery_finalizes_tail_into_enc_and_chain_verifies() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, session, _mirror, total_frames) = record_encrypted_session(tmp.path());
    let key = test_key();

    // ── Имитация краха питания в третьем сегменте ────────────────────────────
    // При настоящем крахе хвостовой сегмент остаётся ОТКРЫТЫМ WAV (усечённый,
    // читаемый — принцип этапа 02) и не успевает ни зашифроваться, ни попасть
    // в журнал. Чистый стоп сделал и то и другое — откатываем: дешифруем
    // хвост обратно в `.wav`, удаляем `.enc`, убираем его segment_completed.
    let enc_files = segment_files_with_suffix(&session, ".wav.enc");
    let last_enc = enc_files.last().unwrap().clone();
    let plain = crypto::read_segment_plain(&last_enc, Some(&key)).unwrap();
    let last_wav = last_enc.with_extension(""); // срезает `.enc` → `….wav`
    std::fs::write(&last_wav, &plain).unwrap();
    std::fs::remove_file(&last_enc).unwrap();
    {
        let jpath = session.join(journal::JOURNAL_FILE_NAME);
        let text = std::fs::read_to_string(&jpath).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        let pos = lines
            .iter()
            .rposition(|l| l.contains("\"segment_completed\""))
            .expect("в журнале есть segment_completed");
        lines.remove(pos);
        std::fs::write(&jpath, lines.join("\n") + "\n").unwrap();
    }
    let before = journal::replay(&session.join(journal::JOURNAL_FILE_NAME)).unwrap();
    assert_eq!(before.completed_segments.len(), 2);
    assert!(before.is_unfinished());

    // ── «Рестарт»: обнаружение + восстановление с дофинализацией ────────────
    let unfinished = recovery::scan_unfinished(session.parent().unwrap()).unwrap();
    assert_eq!(unfinished.len(), 1);
    recovery::recover_in_place(&session, Some(&key)).unwrap();

    // Хвост дофинализирован: открытых WAV нет, все три сегмента — `.enc`.
    assert!(segment_files_with_suffix(&session, ".wav").is_empty());
    assert_eq!(segment_files_with_suffix(&session, ".wav.enc").len(), 3);

    // Журнал дополнен записью хвоста с фактическим stored-именем и кадрами.
    let after = journal::replay(&session.join(journal::JOURNAL_FILE_NAME)).unwrap();
    assert!(after.recovered);
    assert_eq!(after.completed_segments.len(), 3);
    let tail = &after.completed_segments[2];
    assert_eq!(tail.index, 3);
    assert!(tail.path.ends_with(".wav.enc"));
    assert_eq!(tail.frames, 4_000, "хвост: 20000 − 2×8000 кадров");

    // Реконсиляция: статус Recovered, хеш-цепочка сходится (критерий A-2),
    // склейка отдаёт все кадры без потерь.
    let store = ManifestStore::in_memory().unwrap();
    reconcile::reconcile_session(&store, &session, Some(&key))
        .unwrap()
        .unwrap();
    let record = store.get_session("session-1").unwrap().unwrap();
    assert_eq!(
        record.status,
        court_audio_lib::store::manifest::SessionStatus::Recovered
    );
    let segs = store.get_segments("session-1").unwrap();
    assert_eq!(segs.len(), 3);
    let hashes: Vec<String> = segs.iter().map(|s| s.sha256.clone()).collect();
    let links: Vec<String> = segs.iter().map(|s| s.chain_link.clone()).collect();
    assert!(hash::verify_chain(
        &hashes,
        &links,
        record.final_chain_link.as_deref()
    ));

    let tl = Timeline::build(&segs, 0, record.sample_rate_hz);
    let out = tmp.path().join("joined.wav");
    let joined = join_track_to_wav(&tl, Some(&key), &out).unwrap();
    assert_eq!(joined.frames, total_frames as u64);
}

#[test]
fn plaintext_session_reconciles_without_key() {
    // Критерий A-6: записи, сделанные ДО этапа (plaintext WAV в журнале и на
    // диске), реконсилируются и читаются без ключа — миграция не требуется.
    let tmp = tempfile::tempdir().unwrap();
    let session = tmp.path().join("session-old");
    let rate = 8_000u32;

    let signal = ramp(6_000);
    let (producer, consumer) = ring::channel(8_192);
    assert_eq!(producer.push_slice(&signal), signal.len());

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
            operator_id: String::new(),
            station_id: String::new(),
            autonomous_offline: false,
        })
        .unwrap();

    let cfg = ConsumerConfig {
        native_channels: 1,
        target_channels: 1,
        bit_depth: 16,
        sample_rate_hz: rate,
        segment_seconds: 1,
        flush_interval: Duration::from_millis(1_500),
        level_update_hz: 25,
        output_dir: session.clone(),
        scratch_len: consumer.capacity(),
        channel_index: None,
        track_id: 0,
    };
    let rel = ConsumerReliability {
        journal: Some(Arc::clone(&jrnl)),
        mirror: None,
        disk: None,
        max_session: None,
        on_event: None,
        segment_key: None, // storage.encrypt_at_rest = false / запись до 13.7
    };
    let stop = Arc::new(AtomicBool::new(true));
    run_consumer(consumer, cfg, Box::new(|_: LevelEvent| {}), stop, rel).unwrap();
    Journal::open(&session)
        .unwrap()
        .append(&JournalRecord::Stopped)
        .unwrap();

    // Только открытые WAV, ни одного `.enc`; реконсиляция без ключа проходит.
    assert!(segment_files_with_suffix(&session, ".wav.enc").is_empty());
    assert!(!segment_files_with_suffix(&session, ".wav").is_empty());
    let store = ManifestStore::in_memory().unwrap();
    reconcile::reconcile_session(&store, &session, None)
        .unwrap()
        .unwrap();
    let record = store.get_session("session-old").unwrap().unwrap();
    let segs = store.get_segments("session-old").unwrap();
    let hashes: Vec<String> = segs.iter().map(|s| s.sha256.clone()).collect();
    let links: Vec<String> = segs.iter().map(|s| s.chain_link.clone()).collect();
    assert!(hash::verify_chain(
        &hashes,
        &links,
        record.final_chain_link.as_deref()
    ));
}
