//! Структура зеркала дублирующей дорожки (этап 13.3 —
//! `promts/13_3_mirror_structure.md`, R-010). Все — без реального устройства
//! (портируемо в CI):
//!
//! - зеркало **повторяет дерево** основного места (`<mirror>/<session>/<track>/…`);
//!   нет коллизий имён сегментов между дорожками/сессиями;
//! - на зеркале лежат **метаданные** (журнал(ы) + `tracks.json`), и по зеркалу
//!   сессия **реконсилируется независимо** от основного места (самодостаточность);
//! - **сбой зеркала не роняет** основную запись (best-effort).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use court_audio_lib::audio::capture::{
    run_consumer, ConsumerConfig, ConsumerReliability, LevelEvent,
};
use court_audio_lib::audio::ring;
use court_audio_lib::audio::tracks::ResolvedTrack;
use court_audio_lib::integrity::hash;
use court_audio_lib::recorder::journal::{Journal, JournalRecord, JOURNAL_FILE_NAME};
use court_audio_lib::recorder::multitrack::{self, TRACKS_FILE_NAME};
use court_audio_lib::reliability::mirror::Mirror;
use court_audio_lib::store::manifest::{ManifestStore, SessionStatus};
use court_audio_lib::store::reconcile::reconcile_session;

const RATE: u32 = 8_000;
const FRAMES: usize = 16_000; // 2 полных сегмента на дорожку (segment_seconds = 1)

fn ramp(frames: usize) -> Vec<f32> {
    (0..frames).map(|i| (i % 50) as f32 / 100.0).collect()
}

fn track_cfg(dir: PathBuf, track_id: u32) -> ConsumerConfig {
    ConsumerConfig {
        native_channels: 1,
        target_channels: 1,
        bit_depth: 16,
        sample_rate_hz: RATE,
        segment_seconds: 1,
        flush_interval: Duration::from_millis(1_500),
        level_update_hz: 25,
        output_dir: dir,
        scratch_len: FRAMES + 16,
        channel_index: None,
        track_id,
    }
}

fn resolved(track_id: u32, role: &str, ch: u16) -> ResolvedTrack {
    ResolvedTrack {
        track_id,
        device: None,
        channel_index: ch,
        role: role.into(),
        label: role.into(),
    }
}

fn started(channels: u16) -> JournalRecord {
    JournalRecord::SessionStarted {
        started_at_unix_ms: 1_700_000_000_000,
        sample_rate_hz: RATE,
        channels,
        bit_depth: 16,
        segment_seconds: 1,
        operator_id: "op-1".into(),
        station_id: "station-A".into(),
        autonomous_offline: false,
    }
}

/// Записать одну дорожку через consumer (сегменты + журнал зеркалируются «на
/// лету»), затем штатно завершить журнал и досыслать его на зеркало (как IPC).
fn record_track(storage_root: &Path, mirror_root: &Path, tdir: &Path, track_id: u32) {
    let mut tj = Journal::open(tdir).unwrap();
    tj.append(&started(1)).unwrap();
    let track_journal = Arc::new(Mutex::new(tj));

    let (producer, consumer) = ring::channel(FRAMES + 16);
    producer.push_slice(&ramp(FRAMES));

    let rel = ConsumerReliability {
        journal: Some(Arc::clone(&track_journal)),
        mirror: Some(Mirror::new(storage_root, mirror_root).unwrap()),
        disk: None,
        max_session: None,
        on_event: None,
    };
    let stop = Arc::new(AtomicBool::new(true));
    run_consumer(
        consumer,
        track_cfg(tdir.to_path_buf(), track_id),
        Box::new(|_: LevelEvent| {}),
        stop,
        rel,
    )
    .unwrap();

    track_journal
        .lock()
        .unwrap()
        .append(&JournalRecord::Stopped)
        .unwrap();
    Mirror::new(storage_root, mirror_root)
        .unwrap()
        .mirror_file(&tdir.join(JOURNAL_FILE_NAME))
        .unwrap();
}

/// Записать многоканальную сессию целиком: корневой журнал + `tracks.json`
/// (зеркалятся при старте), дорожки (consumer), финал (`Stopped` + досылка).
fn record_session(
    storage_root: &Path,
    mirror_root: &Path,
    name: &str,
    tracks: Vec<ResolvedTrack>,
) -> PathBuf {
    let session_dir = storage_root.join(name);
    let mut root = Journal::open(&session_dir).unwrap();
    root.append(&started(tracks.len() as u16)).unwrap();

    let map = multitrack::track_map_from_resolved(&tracks);
    multitrack::write_track_map(&session_dir, &map).unwrap();

    // Зеркало метаданных при старте (tracks.json + корневой журнал).
    let m = Mirror::new(storage_root, mirror_root).unwrap();
    m.mirror_file(&session_dir.join(TRACKS_FILE_NAME)).unwrap();
    m.mirror_file(&session_dir.join(JOURNAL_FILE_NAME)).unwrap();

    for entry in &map.tracks {
        let tdir = multitrack::track_dir(&session_dir, entry);
        record_track(storage_root, mirror_root, &tdir, entry.track_id);
    }

    root.append(&JournalRecord::Stopped).unwrap();
    Mirror::new(storage_root, mirror_root)
        .unwrap()
        .mirror_file(&session_dir.join(JOURNAL_FILE_NAME))
        .unwrap();
    session_dir
}

#[test]
fn mirror_reconstructs_sessions_and_tracks_independently() {
    let tmp = tempfile::tempdir().unwrap();
    let storage_root = tmp.path().join("recordings");
    let mirror_root = tmp.path().join("mirror");

    // Две сессии: одна многоканальная (2 дорожки), одна одна-дорожка. Обе дорожки
    // имеют seg-0001 — в плоском каталоге имена совпали бы (ядро R-010).
    record_session(
        &storage_root,
        &mirror_root,
        "session-1",
        vec![resolved(0, "judge", 0), resolved(1, "defense", 1)],
    );
    record_session(
        &storage_root,
        &mirror_root,
        "session-2",
        vec![resolved(0, "judge", 0)],
    );

    // Зеркало повторяет дерево: раздельные каталоги сессий/дорожек.
    let m1 = mirror_root.join("session-1");
    let m2 = mirror_root.join("session-2");
    assert!(m1.join("track-00-judge").is_dir());
    assert!(m1.join("track-01-defense").is_dir());
    assert!(m2.join("track-00-judge").is_dir());

    // Метаданные на зеркале присутствуют.
    assert!(m1.join(TRACKS_FILE_NAME).exists());
    assert!(m1.join(JOURNAL_FILE_NAME).exists());
    assert!(m1.join("track-00-judge").join(JOURNAL_FILE_NAME).exists());

    // Нет коллизии: seg-* обеих дорожек лежат в своих подкаталогах.
    let judge_segs = wav_count(&m1.join("track-00-judge"));
    let defense_segs = wav_count(&m1.join("track-01-defense"));
    assert!(judge_segs >= 2 && defense_segs >= 2);

    // Реконсиляция ИЗ ЗЕРКАЛА (не из основного места) даёт консистентную сессию,
    // а пути манифеста указывают на файлы **зеркала** (самодостаточность).
    let store = ManifestStore::in_memory().unwrap();
    reconcile_session(&store, &m1).unwrap();

    let sess = store.get_session("session-1").unwrap().unwrap();
    assert_eq!(sess.status, SessionStatus::Stopped);
    assert_eq!(sess.operator_id, "op-1");
    assert_eq!(sess.station_id, "station-A");

    let tracks = store.get_tracks("session-1").unwrap();
    assert_eq!(tracks.len(), 2);
    for t in &tracks {
        let segs = store.get_track_segments("session-1", t.track_id).unwrap();
        assert!(!segs.is_empty());
        for s in &segs {
            assert!(
                s.path.starts_with(m1.to_str().unwrap()),
                "reconcile из зеркала резолвит на зеркальные файлы: {}",
                s.path
            );
            assert!(Path::new(&s.path).exists());
        }
        // Целостность per-track цепочки пересчитана по зеркальным файлам.
        let hashes: Vec<String> = segs.iter().map(|s| s.sha256.clone()).collect();
        let links: Vec<String> = segs.iter().map(|s| s.chain_link.clone()).collect();
        assert!(hash::verify_chain(&hashes, &links, t.final_chain_link.as_deref()));
    }
}

#[test]
fn mirror_failure_does_not_stop_recording() {
    let tmp = tempfile::tempdir().unwrap();
    let storage_root = tmp.path().join("recordings");
    let session = storage_root.join("session-1");
    std::fs::create_dir_all(&session).unwrap();
    let mirror_root = tmp.path().join("mirror");

    // Зеркало с «неправильным» корнем хранилища: каждая попытка зеркалить сегмент
    // падает (файл вне корня), но основная запись должна пройти полностью.
    let wrong_root = tmp.path().join("some-other-root");
    let rel = ConsumerReliability {
        journal: None,
        mirror: Some(Mirror::new(&wrong_root, &mirror_root).unwrap()),
        disk: None,
        max_session: None,
        on_event: None,
    };

    let (producer, consumer) = ring::channel(FRAMES + 16);
    producer.push_slice(&ramp(FRAMES));
    let stop = Arc::new(AtomicBool::new(true));
    let segments = run_consumer(
        consumer,
        track_cfg(session.clone(), 0),
        Box::new(|_: LevelEvent| {}),
        stop,
        rel,
    )
    .unwrap();

    // Запись цела несмотря на постоянный сбой зеркала.
    assert!(segments.len() >= 2);
    assert!(wav_count(&session) >= 2);
}

fn wav_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.starts_with("seg-") && n.ends_with(".wav"))
                })
                .count()
        })
        .unwrap_or(0)
}
