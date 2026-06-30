//! Обработка очереди выгрузки по сети/токену/приоритету (этап 06 —
//! `promts/06_sync_agent.md`, шаг 5).
//!
//! **Приоритет — захват, не выгрузка.** [`process_queue_once`] — один проход
//! планировщика: гонит [`super::uploader::upload_session`] по всем записям из
//! очереди ([`super::queue::uploadable`]), но уважает:
//! - `sync.auto_upload` — выключено → автоматически ничего не грузим (оператор
//!   запускает выгрузку вручную, минуя планировщик);
//! - `sync.defer_during_recording` — при активной записи тяжёлую догрузку
//!   откладываем, чтобы не мешать захвату.
//!
//! Ошибки отдельных записей **накапливаются**, а не валят проход: одна
//! проблемная запись не блокирует остальные. Бэкофф между проходами задаёт
//! фоновый цикл-вызыватель (в `ipc`/`lib`), используя `sync.retry.*`; сам проход
//! не спит. Чистая, тестируемая на фейк-транспорте функция.

use crate::settings::Settings;
use crate::store::manifest::ManifestStore;
use crate::store::StoreError;

use super::client::UploadTransport;
use super::uploader::{upload_session, UploadOutcome};
use super::{queue, SyncError};

/// Контекст одного прохода планировщика (живой снимок состояния станции).
pub struct TickContext<'a> {
    /// Операторский JWT или `None` (нет токена → копим, не теряем).
    pub token: Option<&'a str>,
    /// Идёт ли сейчас активная запись (приоритет захвата).
    pub is_recording: bool,
    /// Часы для триггера ретеншна.
    pub now_unix_ms: u64,
    /// Настройки станции (реестр).
    pub settings: &'a Settings,
    /// Ключ станции для дешифрования сегментов (нужен для `.enc`).
    pub key: Option<&'a [u8; 32]>,
}

/// Результат прохода: исход выгрузки по каждой обработанной записи.
pub type TickResults = Vec<(String, Result<UploadOutcome, SyncError>)>;

/// Один проход по очереди. Пустой результат — либо очередь пуста, либо проход
/// пропущен (`auto_upload` выключен или отложено на время записи).
pub fn process_queue_once(
    store: &ManifestStore,
    transport: &dyn UploadTransport,
    ctx: &TickContext,
) -> Result<TickResults, StoreError> {
    // Автовыгрузка выключена — планировщик не трогает очередь (только ручной запуск).
    if !ctx.settings.sync.auto_upload {
        return Ok(Vec::new());
    }
    // Идёт запись и настроено откладывание — приоритет захвату.
    if ctx.settings.sync.defer_during_recording && ctx.is_recording {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for session in queue::uploadable(store)? {
        let res = upload_session(
            store,
            transport,
            ctx.token,
            ctx.settings,
            ctx.key,
            &session.id,
            ctx.now_unix_ms,
        );
        out.push((session.id, res));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::events::{EventKind, RecordingEvent};
    use crate::integrity::hash;
    use crate::store::manifest::{SegmentRecord, SessionRecord, SessionStatus};
    use crate::sync::testkit::FakeTransport;
    use std::fs;
    use std::path::Path;

    fn seed_recording(store: &ManifestStore, dir: &Path, id: &str) {
        let mut s = SessionRecord::new(id, dir.to_str().unwrap(), 1, "st", "op", 44_100, 1, 16);
        s.status = SessionStatus::Stopped;
        store.insert_session(&s).unwrap();
        let content = b"seg-content".to_vec();
        let name = "seg-0001.wav";
        fs::write(dir.join(name), &content).unwrap();
        let h = hash::sha256_bytes(&content);
        store
            .append_segment(
                id,
                &SegmentRecord {
                    index: 1,
                    path: name.into(),
                    started_at_unix_ms: 1,
                    frames: 100,
                    size_bytes: content.len() as u64,
                    sha256: h.clone(),
                    chain_link: hash::build_chain(&[h]).pop().unwrap(),
                },
            )
            .unwrap();
        store
            .append_event(id, &RecordingEvent::new(EventKind::Stopped, 1))
            .unwrap();
    }

    fn ctx<'a>(settings: &'a Settings, is_recording: bool) -> TickContext<'a> {
        TickContext {
            token: Some("tok"),
            is_recording,
            now_unix_ms: 1_000,
            settings,
            key: None,
        }
    }

    #[test]
    fn processes_all_queued_recordings() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ManifestStore::in_memory().unwrap();
        let d1 = tmp.path().join("s1");
        let d2 = tmp.path().join("s2");
        fs::create_dir_all(&d1).unwrap();
        fs::create_dir_all(&d2).unwrap();
        seed_recording(&store, &d1, "s1");
        seed_recording(&store, &d2, "s2");

        let t = FakeTransport::happy();
        let settings = Settings::default();
        let results = process_queue_once(&store, &t, &ctx(&settings, false)).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .all(|(_, r)| matches!(r, Ok(UploadOutcome::Verified))));
    }

    #[test]
    fn defers_during_recording_when_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ManifestStore::in_memory().unwrap();
        let d1 = tmp.path().join("s1");
        fs::create_dir_all(&d1).unwrap();
        seed_recording(&store, &d1, "s1");

        let t = FakeTransport::happy();
        let mut settings = Settings::default();
        settings.sync.defer_during_recording = true;

        // Идёт запись → проход пропущен, ничего не отправлено.
        let results = process_queue_once(&store, &t, &ctx(&settings, true)).unwrap();
        assert!(results.is_empty());
        assert_eq!(t.received_count(), 0);

        // Запись не идёт → обрабатываем.
        let results = process_queue_once(&store, &t, &ctx(&settings, false)).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn auto_upload_off_skips_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ManifestStore::in_memory().unwrap();
        let d1 = tmp.path().join("s1");
        fs::create_dir_all(&d1).unwrap();
        seed_recording(&store, &d1, "s1");

        let t = FakeTransport::happy();
        let mut settings = Settings::default();
        settings.sync.auto_upload = false;
        let results = process_queue_once(&store, &t, &ctx(&settings, false)).unwrap();
        assert!(results.is_empty());
        assert_eq!(t.received_count(), 0);
    }

    #[test]
    fn no_token_keeps_queue_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ManifestStore::in_memory().unwrap();
        let d1 = tmp.path().join("s1");
        fs::create_dir_all(&d1).unwrap();
        seed_recording(&store, &d1, "s1");

        let t = FakeTransport::happy();
        let settings = Settings::default();
        let no_token = TickContext {
            token: None,
            ..ctx(&settings, false)
        };
        let results = process_queue_once(&store, &t, &no_token).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].1, Err(SyncError::NoToken)));
        // Запись осталась в очереди, статус не сорван.
        assert!(queue::is_uploadable(
            &store.get_session("s1").unwrap().unwrap()
        ));
    }
}
