//! Возобновляемая чанковая выгрузка одной записи (этап 06 —
//! `promts/06_sync_agent.md`, шаг 3 / deliverables 1–2).
//!
//! Один проход [`upload_session`] по контракту `07`: регистрация → `init` →
//! отправка **неотправленных** частей (часть = сегмент) → `complete` → `verify`.
//! Идемпотентность и докачка — через трекинг частей в [`super::queue`]: после
//! обрыва следующий проход шлёт только `pending`, повтор `part/<n>` безопасен.
//! Часть читается из хранилища с прозрачным дешифрованием
//! ([`crate::store::crypto::read_segment_plain`]); хеши/состав — из манифеста
//! ([`crate::store::export::build_manifest`]).
//!
//! **Бэкофф между проходами** делает планировщик ([`super::scheduler`]): один
//! проход = одна сетевая попытка на часть; временный сбой оставляет запись в
//! очереди (`Uploading`), не теряя прогресс; постоянный (4xx) → `Failed`.
//! Параллельность частей — до `sync.parallel_uploads` (через `thread::scope`),
//! запись в манифест — последовательная, без шаринга SQLite между потоками.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::settings::Settings;
use crate::store::crypto::read_segment_plain;
use crate::store::export::build_manifest;
use crate::store::manifest::{ManifestStore, UploadStatus};

use super::client::{SessionMeta, UploadTransport};
use super::{queue, verify, SyncError};

/// Байт в мегабайте — перевод `sync.chunk_size_mb` в предел размера части.
const BYTES_PER_MB: u64 = 1024 * 1024;

/// Итог одного прохода выгрузки записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadOutcome {
    /// Все части приняты, `complete` + `verify` прошли, целостность подтверждена
    /// (триггер ретеншна дёрнут).
    Verified,
    /// `verify` вернул несовпадение целостности — копия сохранена, статус ошибки.
    IntegrityFailed,
}

/// Идентичность для регистрации сессии: значение из записи, иначе — из env
/// (временный seam до экрана входа оператора). Пустая env-переменная
/// игнорируется. См. [`super::OPERATOR_ID_ENV`] / [`super::STATION_ID_ENV`].
fn identity_or_env(value: &str, env_key: &str) -> String {
    if !value.is_empty() {
        return value.to_string();
    }
    std::env::var(env_key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_default()
}

/// Выгрузить запись `session_id` за один проход. `token = None` → [`SyncError::NoToken`]
/// (копим в очереди, не теряем). `key` — ключ станции для дешифрования сегментов
/// (нужен только для `.enc`). `now_unix_ms` — часы для триггера ретеншна.
pub fn upload_session(
    store: &ManifestStore,
    transport: &dyn UploadTransport,
    token: Option<&str>,
    settings: &Settings,
    key: Option<&[u8; 32]>,
    session_id: &str,
    now_unix_ms: u64,
) -> Result<UploadOutcome, SyncError> {
    let token = token.ok_or(SyncError::NoToken)?;

    let session = store
        .get_session(session_id)?
        .ok_or_else(|| SyncError::Permanent(format!("сессия {session_id} не найдена")))?;

    let manifest = build_manifest(
        store,
        session_id,
        &settings.integrity.segment_hash,
        settings.integrity.hash_chain,
    )?;
    if manifest.segments.is_empty() {
        return Err(SyncError::Permanent(
            "у записи нет сегментов для выгрузки".to_string(),
        ));
    }

    store.set_upload_status(session_id, UploadStatus::Uploading)?;
    queue::init_parts_from_manifest(store, session_id, &manifest)?;

    // 1. Регистрация сессии (идемпотентно: повторно не регистрируем).
    let recording_id = match queue::get_recording_id(store, session_id)? {
        Some(id) => id,
        None => {
            let meta = SessionMeta {
                session_id: session.id.clone(),
                // До экрана входа оператора идентичность пустая в записи сессии
                // (reconcile), поэтому добираем её из env (как и токен). Сервер
                // `07` требует числовой operator_id (PK пользователя ex_system).
                station_id: identity_or_env(&session.station_id, super::STATION_ID_ENV),
                operator_id: identity_or_env(&session.operator_id, super::OPERATOR_ID_ENV),
                adjudication_ref: session.adjudication_ref.clone(),
                sample_rate_hz: session.sample_rate_hz,
                channels: session.channels,
                bit_depth: session.bit_depth,
            };
            let id = transport.register_session(token, &meta)?;
            queue::set_recording_id(store, session_id, &id)?;
            id
        }
    };

    // 2. Заявка состава сегментов (один раз).
    if !queue::is_init_done(store, session_id)? {
        transport.init_upload(token, &recording_id, &manifest.segments)?;
        queue::mark_init_done(store, session_id)?;
    }

    // 3. Отправка неотправленных частей батчами до parallel_uploads.
    let pending = queue::pending_parts(store, session_id)?;
    if !pending.is_empty() {
        let path_by_index = segment_paths(store, session_id, &session.dir)?;
        let parallel = settings.sync.parallel_uploads.max(1) as usize;
        let max_part_bytes = (settings.sync.chunk_size_mb as u64).saturating_mul(BYTES_PER_MB);

        for batch in pending.chunks(parallel) {
            let results = upload_batch(
                transport,
                token,
                &recording_id,
                batch,
                &path_by_index,
                key,
                max_part_bytes,
            );

            let mut had_transient = false;
            for (part_index, res) in results {
                match res {
                    Ok(()) => queue::mark_part_sent(store, session_id, part_index)?,
                    Err(SyncError::Permanent(msg)) => {
                        queue::record_part_attempt(store, session_id, part_index, &msg)?;
                        store.set_upload_status(session_id, UploadStatus::Failed)?;
                        return Err(SyncError::Permanent(msg));
                    }
                    Err(e) => {
                        queue::record_part_attempt(store, session_id, part_index, &e.to_string())?;
                        had_transient = true;
                    }
                }
            }
            if had_transient {
                // Прогресс сохранён; запись остаётся в очереди — докачаем позже.
                return Err(SyncError::Transient(
                    "часть не принята, докачка отложена".to_string(),
                ));
            }
        }
    }

    // 4. Финализация и верификация.
    transport.complete_upload(token, &recording_id)?;
    store.set_upload_status(session_id, UploadStatus::Uploaded)?;
    let verified = verify::run_verify(
        transport,
        token,
        store,
        &recording_id,
        session_id,
        now_unix_ms,
    )?;
    Ok(if verified {
        UploadOutcome::Verified
    } else {
        UploadOutcome::IntegrityFailed
    })
}

/// Отправить батч частей параллельно (сеть), вернуть результат по каждой.
/// SQLite между потоками не шарим — запись в манифест делает вызывающий.
fn upload_batch(
    transport: &dyn UploadTransport,
    token: &str,
    recording_id: &str,
    batch: &[queue::PartRow],
    path_by_index: &HashMap<u32, PathBuf>,
    key: Option<&[u8; 32]>,
    max_part_bytes: u64,
) -> Vec<(u32, Result<(), SyncError>)> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = batch
            .iter()
            .map(|part| {
                scope.spawn(move || {
                    let res = upload_one_part(
                        transport,
                        token,
                        recording_id,
                        part,
                        path_by_index.get(&part.part_index),
                        key,
                        max_part_bytes,
                    );
                    (part.part_index, res)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("поток выгрузки части не должен паниковать"))
            .collect()
    })
}

/// Прочитать и отправить одну часть. Локальные проблемы (нет файла/слишком
/// крупная часть/ошибка дешифрования) — постоянные (ретрай не поможет).
fn upload_one_part(
    transport: &dyn UploadTransport,
    token: &str,
    recording_id: &str,
    part: &queue::PartRow,
    stored_path: Option<&PathBuf>,
    key: Option<&[u8; 32]>,
    max_part_bytes: u64,
) -> Result<(), SyncError> {
    let path = stored_path.ok_or_else(|| {
        SyncError::Permanent(format!("нет файла сегмента для части {}", part.part_index))
    })?;
    if part.size_bytes > max_part_bytes {
        return Err(SyncError::Permanent(format!(
            "часть {} ({} Б) больше предела sync.chunk_size_mb ({} Б)",
            part.part_index, part.size_bytes, max_part_bytes
        )));
    }
    let bytes = read_segment_plain(path, key)
        .map_err(|e| SyncError::Permanent(format!("чтение сегмента {}: {e}", part.part_index)))?;
    transport.upload_part(token, recording_id, part.part_index, &bytes)?;
    Ok(())
}

/// Карта индекс сегмента → абсолютный путь хранимого файла.
fn segment_paths(
    store: &ManifestStore,
    session_id: &str,
    session_dir: &str,
) -> Result<HashMap<u32, PathBuf>, SyncError> {
    let dir = PathBuf::from(session_dir);
    Ok(store
        .get_segments(session_id)?
        .into_iter()
        .map(|s| (s.index, resolve_segment_path(&dir, &s.path)))
        .collect())
}

/// Абсолютный путь сегмента: относительный — относительно каталога сессии
/// (как в [`crate::store::retention`]).
fn resolve_segment_path(session_dir: &Path, stored: &str) -> PathBuf {
    let p = Path::new(stored);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        session_dir.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::events::{EventKind, RecordingEvent};
    use crate::integrity::hash;
    use crate::store::manifest::{SegmentRecord, SessionRecord, SessionStatus};
    use crate::sync::testkit::{FakeConfig, FakeTransport};
    use std::fs;

    #[test]
    fn identity_prefers_value_then_env() {
        // Уникальный ключ env, чтобы не пересекаться с другими тестами.
        let key = "COURT_AUDIO_TEST_OPERATOR_ID_XYZ";
        // Непустое значение записи — env не трогаем.
        assert_eq!(identity_or_env("op-7", key), "op-7");
        // Пусто в записи → берём из env.
        std::env::set_var(key, "42");
        assert_eq!(identity_or_env("", key), "42");
        // Нет env → пусто (сервер тогда корректно отклонит 400).
        std::env::remove_var(key);
        assert_eq!(identity_or_env("", key), "");
    }

    /// Подготовить запись с `n` сегментами на диске (без шифрования) + манифест.
    fn seed_recording(dir: &Path, n: u32) -> ManifestStore {
        let store = ManifestStore::in_memory().unwrap();
        let mut s = SessionRecord::new(
            "s1",
            dir.to_str().unwrap(),
            1,
            "station-1",
            "operator-7",
            44_100,
            1,
            16,
        );
        s.status = SessionStatus::Stopped;
        store.insert_session(&s).unwrap();

        let mut hashes = Vec::new();
        for i in 1..=n {
            let content = format!("segment-{i}-content").into_bytes();
            let name = format!("seg-{i:04}.wav");
            fs::write(dir.join(&name), &content).unwrap();
            hashes.push(hash::sha256_bytes(&content));
            store
                .append_segment(
                    "s1",
                    &SegmentRecord {
                        index: i,
                        path: name,
                        started_at_unix_ms: i as u64,
                        frames: 100,
                        size_bytes: content.len() as u64,
                        sha256: hashes[(i - 1) as usize].clone(),
                        chain_link: format!("link{i}"),
                    },
                )
                .unwrap();
        }
        let chain = hash::build_chain(&hashes);
        store
            .set_final_chain_link("s1", chain.last().unwrap())
            .unwrap();
        store
            .append_event("s1", &RecordingEvent::new(EventKind::Stopped, 1))
            .unwrap();
        store
    }

    fn settings() -> Settings {
        // Дефолты реестра: chunk 8 МБ, parallel 1.
        Settings::default()
    }

    #[test]
    fn no_token_accumulates_without_loss() {
        let tmp = tempfile::tempdir().unwrap();
        let store = seed_recording(tmp.path(), 2);
        let t = FakeTransport::happy();
        let res = upload_session(&store, &t, None, &settings(), None, "s1", 1);
        assert!(matches!(res, Err(SyncError::NoToken)));
        // Ничего не отправлено, запись осталась в очереди.
        assert_eq!(t.received_count(), 0);
        assert!(queue::is_uploadable(
            &store.get_session("s1").unwrap().unwrap()
        ));
    }

    #[test]
    fn happy_path_uploads_verifies_and_confirms() {
        let tmp = tempfile::tempdir().unwrap();
        let store = seed_recording(tmp.path(), 3);
        let t = FakeTransport::happy();
        let out = upload_session(&store, &t, Some("tok"), &settings(), None, "s1", 5_000).unwrap();
        assert_eq!(out, UploadOutcome::Verified);
        assert_eq!(t.received_count(), 3);
        assert_eq!(t.complete_calls(), 1);
        let s = store.get_session("s1").unwrap().unwrap();
        assert_eq!(s.upload_status, UploadStatus::Confirmed);
        assert_eq!(s.confirmed_at_unix_ms, Some(5_000));
    }

    #[test]
    fn resumes_from_middle_after_transient_break() {
        let tmp = tempfile::tempdir().unwrap();
        let store = seed_recording(tmp.path(), 4);
        // Часть 3 один раз обрывается посреди выгрузки.
        let t = FakeTransport::new(FakeConfig {
            verify_result: true,
            fail_parts_transient_once: vec![3],
            ..Default::default()
        });

        // Первый проход: части 1,2 уходят, на 3 обрыв → Transient.
        let first = upload_session(&store, &t, Some("tok"), &settings(), None, "s1", 1);
        assert!(matches!(first, Err(SyncError::Transient(_))));
        assert!(t.has_part(1) && t.has_part(2));
        assert!(!t.has_part(3) && !t.has_part(4));
        // Запись всё ещё в очереди, статус Uploading.
        let s = store.get_session("s1").unwrap().unwrap();
        assert_eq!(s.upload_status, UploadStatus::Uploading);
        assert_eq!(
            queue::progress(&store, "s1").unwrap(),
            queue::PartProgress { total: 4, sent: 2 }
        );

        // Второй проход: докачивает только 3 и 4, без повторной отправки 1,2.
        let second =
            upload_session(&store, &t, Some("tok"), &settings(), None, "s1", 9_000).unwrap();
        assert_eq!(second, UploadOutcome::Verified);
        // Каждая часть принята ровно один раз (идемпотентность множества).
        assert_eq!(t.received_count(), 4);
        assert_eq!(t.complete_calls(), 1);
    }

    #[test]
    fn permanent_part_error_fails_without_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let store = seed_recording(tmp.path(), 2);
        let t = FakeTransport::new(FakeConfig {
            verify_result: true,
            fail_parts_permanent: vec![2],
            ..Default::default()
        });
        let res = upload_session(&store, &t, Some("tok"), &settings(), None, "s1", 1);
        assert!(matches!(res, Err(SyncError::Permanent(_))));
        let s = store.get_session("s1").unwrap().unwrap();
        assert_eq!(s.upload_status, UploadStatus::Failed);
        // complete не вызывался — выгрузка не финализирована.
        assert_eq!(t.complete_calls(), 0);
    }

    #[test]
    fn verify_false_keeps_local_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let store = seed_recording(tmp.path(), 1);
        let t = FakeTransport::new(FakeConfig {
            verify_result: false,
            ..Default::default()
        });
        let out = upload_session(&store, &t, Some("tok"), &settings(), None, "s1", 1).unwrap();
        assert_eq!(out, UploadOutcome::IntegrityFailed);
        let s = store.get_session("s1").unwrap().unwrap();
        assert_eq!(s.upload_status, UploadStatus::IntegrityFailed);
        assert_eq!(s.confirmed_at_unix_ms, None);
    }
}
