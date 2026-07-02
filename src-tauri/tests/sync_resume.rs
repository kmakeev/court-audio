//! Интеграционный тест агента выгрузки (этап 06 — `promts/06_sync_agent.md`).
//!
//! Полный цикл на собственном фейк-транспорте (сервера `07` в этом репозитории
//! нет, как и сетевого seam'а этапа 05): `init` → части → **обрыв** → докачка с
//! середины → `complete` → `verify` → **триггер ретеншна**. Плюс
//! персистентность очереди через «рестарт» (переоткрытие манифеста на диске).

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use court_audio_lib::integrity::events::{EventKind, RecordingEvent};
use court_audio_lib::integrity::hash;
use court_audio_lib::settings::{RetentionMode, Settings};
use court_audio_lib::store::export::{AnnotationsExport, TrackEntry};
use court_audio_lib::store::manifest::{
    ManifestStore, SegmentRecord, SessionRecord, SessionStatus, UploadStatus,
};
use court_audio_lib::store::retention::{self, RetentionPolicy};
use court_audio_lib::sync::client::{SessionMeta, TransportError, UploadTransport, VerifyOutcome};
use court_audio_lib::sync::queue::{self, PartProgress};
use court_audio_lib::sync::uploader::{upload_session, UploadOutcome};

/// Фейк-транспорт интеграционного теста: одноразовый обрыв заданной части.
struct FlakyTransport {
    fail_once: HashSet<u32>,
    used: Mutex<HashSet<u32>>,
    received: Mutex<HashSet<u32>>,
    completes: Mutex<u32>,
}

impl FlakyTransport {
    fn new(fail_once: impl IntoIterator<Item = u32>) -> Self {
        Self {
            fail_once: fail_once.into_iter().collect(),
            used: Mutex::new(HashSet::new()),
            received: Mutex::new(HashSet::new()),
            completes: Mutex::new(0),
        }
    }
}

impl UploadTransport for FlakyTransport {
    fn register_session(
        &self,
        _token: &str,
        _meta: &SessionMeta,
    ) -> Result<String, TransportError> {
        Ok("rec-int-1".to_string())
    }

    fn init_upload(
        &self,
        _token: &str,
        _recording_id: &str,
        _tracks: &[TrackEntry],
        _annotations: &AnnotationsExport,
    ) -> Result<(), TransportError> {
        Ok(())
    }

    fn upload_part(
        &self,
        _token: &str,
        _recording_id: &str,
        _track_id: u32,
        part_index: u32,
        _bytes: &[u8],
    ) -> Result<(), TransportError> {
        let mut used = self.used.lock().unwrap();
        if self.fail_once.contains(&part_index) && !used.contains(&part_index) {
            used.insert(part_index);
            return Err(TransportError::transient("обрыв сети"));
        }
        self.received.lock().unwrap().insert(part_index);
        Ok(())
    }

    fn complete_upload(&self, _token: &str, _recording_id: &str) -> Result<(), TransportError> {
        *self.completes.lock().unwrap() += 1;
        Ok(())
    }

    fn verify(&self, _token: &str, _recording_id: &str) -> Result<VerifyOutcome, TransportError> {
        Ok(VerifyOutcome {
            integrity_verified: true,
        })
    }
}

/// Подготовить на диске запись из `n` сегментов (без шифрования) + манифест.
fn seed_recording(store: &ManifestStore, dir: &Path, n: u32) {
    let mut s = SessionRecord::new(
        "sess-int",
        dir.to_str().unwrap(),
        1_700_000_000_000,
        "station-int",
        "operator-int",
        44_100,
        1,
        16,
    );
    s.status = SessionStatus::Stopped;
    store.insert_session(&s).unwrap();

    let mut hashes = Vec::new();
    for i in 1..=n {
        let content = format!("integration-segment-{i}").into_bytes();
        let name = format!("seg-{i:04}.wav");
        fs::write(dir.join(&name), &content).unwrap();
        let h = hash::sha256_bytes(&content);
        hashes.push(h.clone());
        store
            .append_segment(
                "sess-int",
                &SegmentRecord {
                    track_id: 0,
                    index: i,
                    path: name,
                    started_at_unix_ms: i as u64,
                    frames: 1000,
                    size_bytes: content.len() as u64,
                    sha256: h,
                    chain_link: format!("link{i}"),
                },
            )
            .unwrap();
    }
    let chain = hash::build_chain(&hashes);
    store
        .set_final_chain_link("sess-int", chain.last().unwrap())
        .unwrap();
    store
        .append_event("sess-int", &RecordingEvent::new(EventKind::Stopped, 1))
        .unwrap();
}

#[test]
fn full_cycle_resumes_after_break_and_triggers_retention() {
    let tmp = tempfile::tempdir().unwrap();
    let manifest_path = tmp.path().join("manifest.sqlite");
    let dir = tmp.path().join("sess-int");
    fs::create_dir_all(&dir).unwrap();

    // Часть 3 обрывается один раз.
    let transport = FlakyTransport::new([3]);
    let settings = Settings::default();

    // Проход 1: на диске манифест; обрыв на части 3 → Transient, прогресс сохранён.
    {
        let store = ManifestStore::open(&manifest_path).unwrap();
        seed_recording(&store, &dir, 5);
        let r1 = upload_session(
            &store,
            &transport,
            Some("tok"),
            &settings,
            None,
            "sess-int",
            1,
        );
        assert!(r1.is_err(), "первый проход должен оборваться");
        assert_eq!(
            queue::progress(&store, "sess-int").unwrap(),
            PartProgress { total: 5, sent: 2 }
        );
    } // store закрыт — имитируем рестарт приложения.

    // Проход 2: переоткрываем манифест с диска (очередь пережила рестарт) и
    // докачиваем остаток → complete → verify=true → триггер ретеншна.
    {
        let store = ManifestStore::open(&manifest_path).unwrap();
        // Очередь восстановлена из персистентного манифеста.
        assert!(queue::is_uploadable(
            &store.get_session("sess-int").unwrap().unwrap()
        ));
        let confirmed_at = 9_000_000u64;
        let out = upload_session(
            &store,
            &transport,
            Some("tok"),
            &settings,
            None,
            "sess-int",
            confirmed_at,
        )
        .unwrap();
        assert_eq!(out, UploadOutcome::Verified);

        // Каждая часть принята ровно один раз (идемпотентность докачки).
        assert_eq!(transport.received.lock().unwrap().len(), 5);
        assert_eq!(*transport.completes.lock().unwrap(), 1);

        // Триггер ретеншна сработал: запись подтверждена.
        let s = store.get_session("sess-int").unwrap().unwrap();
        assert_eq!(s.upload_status, UploadStatus::Confirmed);
        assert!(s.server_integrity_verified);
        assert_eq!(s.confirmed_at_unix_ms, Some(confirmed_at));

        // По истечении окна безопасности ретеншн удалит локальную копию.
        let policy = RetentionPolicy::from_settings(&settings.retention);
        assert_eq!(
            settings.retention.mode,
            RetentionMode::UntilConfirmedPlusWindow
        );
        let window_ms = settings.retention.safety_window_hours as u64 * 3_600_000;
        let purged =
            retention::sweep(&store, &settings.retention, confirmed_at + window_ms).unwrap();
        assert_eq!(purged, vec!["sess-int".to_string()]);
        // Сегменты удалены с диска, сессия осталась tombstone'ом.
        assert!(!dir.join("seg-0001.wav").exists());
        assert_eq!(
            store.get_session("sess-int").unwrap().unwrap().status,
            SessionStatus::Purged
        );
        let _ = policy;
    }
}
