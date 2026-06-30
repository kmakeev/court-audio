//! Реконсиляция журнал→SQLite при восстановлении (этап 03 —
//! `promts/03_store_integrity.md`, шаг 8).
//!
//! Аварийно-устойчивый журнал этапа 02 — «последняя инстанция» состояния
//! сессии. При восстановлении ([`crate::recorder::recovery`]) реплеим журнал и
//! приводим SQLite-манифест в соответствие: сессия, сегменты (с пересчётом
//! SHA-256 и хеш-цепочки по файлам — журнал хеши не хранит) и значимые события.
//! Идемпотентно: повторный прогон не плодит дублей (upsert сегментов,
//! ensure-семантика событий).

use std::path::{Path, PathBuf};

use super::manifest::{ManifestStore, SegmentRecord, SessionRecord, SessionStatus};
use super::StoreError;
use crate::integrity::events::{EventKind, RecordingEvent};
use crate::integrity::hash;
use crate::recorder::journal::{self, CompletedSegment};

/// Реконсилировать сессию из её каталога. Возвращает id реконсилированной сессии
/// или `None`, если каталог не содержит начатой сессии.
pub fn reconcile_session(store: &ManifestStore, dir: &Path) -> Result<Option<String>, StoreError> {
    let journal_path = dir.join(journal::JOURNAL_FILE_NAME);
    if !journal_path.exists() {
        return Ok(None);
    }
    let state = journal::replay(&journal_path)?;
    let Some(meta) = state.started.clone() else {
        return Ok(None);
    };

    let id = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| StoreError::Io(format!("некорректный каталог сессии: {dir:?}")))?
        .to_string();
    let dir_str = dir.to_string_lossy().into_owned();

    let existing = store.get_session(&id)?;
    // Уже удалена ретеншном — не воскрешаем.
    if let Some(s) = &existing {
        if s.status == SessionStatus::Purged {
            return Ok(Some(id));
        }
    }

    let status = if state.recovered {
        SessionStatus::Recovered
    } else if state.stopped {
        SessionStatus::Stopped
    } else {
        SessionStatus::Recording
    };

    // База — существующая запись (сохраняем поля выгрузки/привязки/подтверждения),
    // иначе новая со станцией/оператором «неизвестно» (уточнятся при ресюме/UI).
    let mut rec = existing.unwrap_or_else(|| {
        SessionRecord::new(
            &id,
            &dir_str,
            meta.started_at_unix_ms,
            "",
            "",
            meta.sample_rate_hz,
            meta.channels,
            meta.bit_depth,
        )
    });
    rec.dir = dir_str;
    rec.started_at_unix_ms = meta.started_at_unix_ms;
    rec.sample_rate_hz = meta.sample_rate_hz;
    rec.channels = meta.channels;
    rec.bit_depth = meta.bit_depth;
    rec.status = status;
    store.insert_session(&rec)?;

    reconcile_segments(store, &id, dir, &state.completed_segments)?;
    reconcile_events(store, &id, &meta.started_at_unix_ms, &state)?;

    Ok(Some(id))
}

/// Пересчитать SHA-256 и хеш-цепочку по файлам завершённых сегментов и записать
/// их в манифест. Сегменты упорядочиваются по индексу.
fn reconcile_segments(
    store: &ManifestStore,
    session_id: &str,
    dir: &Path,
    completed: &[CompletedSegment],
) -> Result<(), StoreError> {
    let mut segs: Vec<&CompletedSegment> = completed.iter().collect();
    segs.sort_by_key(|s| s.index);

    let mut prev_link: Option<String> = None;
    for seg in segs {
        let path = resolve_path(dir, &seg.path);
        let sha256 = hash::sha256_file(&path)?;
        let chain_link = hash::chain_link(prev_link.as_deref(), &sha256);
        let size_bytes = std::fs::metadata(&path)?.len();
        store.append_segment(
            session_id,
            &SegmentRecord {
                index: seg.index,
                path: path.to_string_lossy().into_owned(),
                started_at_unix_ms: 0, // журнал не хранит таймкод сегмента (этап 02)
                frames: seg.frames,
                size_bytes,
                sha256,
                chain_link: chain_link.clone(),
            },
        )?;
        prev_link = Some(chain_link);
    }

    if let Some(final_link) = prev_link {
        store.set_final_chain_link(session_id, &final_link)?;
    }
    Ok(())
}

/// Восстановить значимые события (ensure-семантика: вставляем только
/// отсутствующие — идемпотентно).
fn reconcile_events(
    store: &ManifestStore,
    session_id: &str,
    started_at_unix_ms: &u64,
    state: &journal::ReplayState,
) -> Result<(), StoreError> {
    ensure_event(
        store,
        session_id,
        EventKind::SessionStarted,
        *started_at_unix_ms,
    )?;
    if state.recovered {
        ensure_event(store, session_id, EventKind::Recovered, *started_at_unix_ms)?;
    }
    if state.stopped {
        ensure_event(store, session_id, EventKind::Stopped, *started_at_unix_ms)?;
    }
    Ok(())
}

/// Вставить событие, если события такого `kind` ещё нет у сессии.
fn ensure_event(
    store: &ManifestStore,
    session_id: &str,
    kind: EventKind,
    at_unix_ms: u64,
) -> Result<(), StoreError> {
    let exists = store
        .get_events(session_id)?
        .iter()
        .any(|e| e.event.kind == kind);
    if !exists {
        store.append_event(session_id, &RecordingEvent::new(kind, at_unix_ms))?;
    }
    Ok(())
}

fn resolve_path(dir: &Path, stored: &str) -> PathBuf {
    let p = Path::new(stored);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        dir.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::journal::{Journal, JournalRecord};
    use crate::recorder::segment_writer::{SegmentConfig, SegmentWriter};
    use std::time::Duration;

    /// Записать настоящий WAV-сегмент в каталог и вернуть имя файла + кадры.
    fn write_segment(dir: &Path, index_frames: usize) -> (String, u64) {
        let cfg = SegmentConfig {
            dir: dir.to_path_buf(),
            sample_rate_hz: 8_000,
            channels: 1,
            bits_per_sample: 16,
            segment_seconds: 3_600,
            flush_interval: Duration::from_millis(1_500),
        };
        let mut w = SegmentWriter::new(cfg).unwrap();
        let samples: Vec<i16> = (0..index_frames).map(|i| (i % 50) as i16).collect();
        w.write_samples(&samples).unwrap();
        let segs = w.finalize().unwrap();
        let path = segs[0].path.clone();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        (name, segs[0].frames)
    }

    fn build_session_dir(root: &Path, name: &str, stopped: bool) -> (PathBuf, Vec<String>) {
        let dir = root.join(name);
        let mut journal = Journal::open(&dir).unwrap();
        journal
            .append(&JournalRecord::SessionStarted {
                started_at_unix_ms: 1_700_000_000_000,
                sample_rate_hz: 8_000,
                channels: 1,
                bit_depth: 16,
                segment_seconds: 30,
            })
            .unwrap();
        let mut files = Vec::new();
        for index in 1..=2u32 {
            let (file_name, frames) = write_segment(&dir, 100 * index as usize);
            journal
                .append(&JournalRecord::SegmentCompleted {
                    index,
                    path: file_name.clone(),
                    frames,
                })
                .unwrap();
            files.push(file_name);
        }
        if stopped {
            journal.append(&JournalRecord::Stopped).unwrap();
        }
        (dir, files)
    }

    #[test]
    fn reconciles_session_segments_and_chain() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _files) = build_session_dir(tmp.path(), "sess-1", true);
        let store = ManifestStore::in_memory().unwrap();

        let id = reconcile_session(&store, &dir).unwrap().unwrap();
        assert_eq!(id, "sess-1");

        let session = store.get_session("sess-1").unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Stopped);
        assert_eq!(session.sample_rate_hz, 8_000);

        let segs = store.get_segments("sess-1").unwrap();
        assert_eq!(segs.len(), 2);
        // Цепочка пересчитана по файлам и согласована.
        let hashes: Vec<String> = segs.iter().map(|s| s.sha256.clone()).collect();
        let links: Vec<String> = segs.iter().map(|s| s.chain_link.clone()).collect();
        assert!(hash::verify_chain(
            &hashes,
            &links,
            session.final_chain_link.as_deref()
        ));
    }

    #[test]
    fn reconcile_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _files) = build_session_dir(tmp.path(), "sess-1", true);
        let store = ManifestStore::in_memory().unwrap();

        reconcile_session(&store, &dir).unwrap();
        let first_segs = store.get_segments("sess-1").unwrap();
        let first_events = store.get_events("sess-1").unwrap();

        // Повторный прогон не плодит дублей.
        reconcile_session(&store, &dir).unwrap();
        let second_segs = store.get_segments("sess-1").unwrap();
        let second_events = store.get_events("sess-1").unwrap();
        assert_eq!(first_segs, second_segs);
        assert_eq!(first_events.len(), second_events.len());
    }

    #[test]
    fn unfinished_session_marked_recording() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _files) = build_session_dir(tmp.path(), "sess-1", false);
        let store = ManifestStore::in_memory().unwrap();
        reconcile_session(&store, &dir).unwrap();
        assert_eq!(
            store.get_session("sess-1").unwrap().unwrap().status,
            SessionStatus::Recording
        );
    }

    #[test]
    fn no_journal_yields_none() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ManifestStore::in_memory().unwrap();
        assert!(reconcile_session(&store, tmp.path()).unwrap().is_none());
    }
}
