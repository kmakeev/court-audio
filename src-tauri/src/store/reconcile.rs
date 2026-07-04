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

use super::manifest::{ManifestStore, SegmentRecord, SessionRecord, SessionStatus, TrackRecord};
use super::StoreError;
use crate::integrity::events::{EventKind, RecordingEvent};
use crate::integrity::hash;
use crate::recorder::journal::{self, CompletedSegment};
use crate::recorder::multitrack;

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

    let status = if state.recovered {
        SessionStatus::Recovered
    } else if state.stopped {
        SessionStatus::Stopped
    } else {
        SessionStatus::Recording
    };

    let existing = store.get_session(&id)?;
    if let Some(s) = &existing {
        // Уже удалена ретеншном — не воскрешаем.
        if s.status == SessionStatus::Purged {
            return Ok(Some(id));
        }
        // Терминальная сессия с тем же числом сегментов уже полностью
        // реконсилирована: ничего не изменилось — пропускаем дорогой пересчёт
        // SHA-256 по всем сегментам. Без этого каждый показ списка сессий и
        // диагностики пере-хешировал бы все записи (видимые паузы интерфейса).
        if s.status == status
            && matches!(status, SessionStatus::Stopped | SessionStatus::Recovered)
            && store.get_segments(&id)?.len() == state.completed_segments.len()
        {
            return Ok(Some(id));
        }
    }

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
    // Идентичность (этап 10.3): журнал — write-ahead источник истины. Проставляем
    // из `SessionStarted`, когда в записи ещё пусто (не затираем уже известное —
    // напр. смену оператора/уточнение в UI на будущее).
    if rec.operator_id.is_empty() && !meta.operator_id.is_empty() {
        rec.operator_id = meta.operator_id.clone();
    }
    if rec.station_id.is_empty() && !meta.station_id.is_empty() {
        rec.station_id = meta.station_id.clone();
    }
    // Автономный офлайн-старт (B-001, этап 13.6): журнал — write-ahead источник.
    // Ставим признак, когда журнал его несёт (не затираем уже помеченную запись).
    if meta.autonomous_offline {
        rec.autonomous_offline = true;
    }
    rec.status = status;
    store.insert_session(&rec)?;

    // Многоканал (этап 09): при наличии карты дорожек сегменты живут в
    // подкаталогах дорожек — реконсилируем per-track. Иначе — одноканальный
    // путь v1 (сегменты из корневого журнала).
    if let Some(map) = multitrack::read_track_map(dir) {
        reconcile_tracks(store, &id, dir, &map)?;
    } else {
        reconcile_segments(store, &id, dir, &state.completed_segments)?;
    }
    reconcile_events(store, &id, &meta.started_at_unix_ms, &state)?;
    reconcile_annotations(store, &id, &state.annotations)?;

    Ok(Some(id))
}

/// Реконсилировать живую разметку (этап 10) из журнала в SQLite. `chain_link`
/// вычислен при постановке — не пере-хешируем; вставка идемпотентна (по `seq`),
/// поэтому повторный прогон дублей не плодит. Так метки/интервалы переживают
/// рестарт (журнал — write-ahead «последняя инстанция»).
fn reconcile_annotations(
    store: &ManifestStore,
    session_id: &str,
    annotations: &[crate::integrity::annotations::AnnotationRecord],
) -> Result<(), StoreError> {
    for rec in annotations {
        store.append_annotation(session_id, rec)?;
    }
    Ok(())
}

/// Пересчитать SHA-256 и хеш-цепочку по файлам завершённых сегментов
/// одноканальной (v1) сессии и зафиксировать финальное звено сессии.
fn reconcile_segments(
    store: &ManifestStore,
    session_id: &str,
    dir: &Path,
    completed: &[CompletedSegment],
) -> Result<(), StoreError> {
    let final_link = reconcile_track_segments(store, session_id, 0, dir, completed)?;
    if let Some(final_link) = final_link {
        store.set_final_chain_link(session_id, &final_link)?;
    }
    Ok(())
}

/// Пересчитать SHA-256 и хеш-цепочку сегментов **одной дорожки** `track_id`
/// (per-track целостность, этап 09). Возвращает финальное звено цепочки дорожки.
/// Известные сегменты не пере-хешируются (дорого) — берём сохранённое звено.
fn reconcile_track_segments(
    store: &ManifestStore,
    session_id: &str,
    track_id: u32,
    dir: &Path,
    completed: &[CompletedSegment],
) -> Result<Option<String>, StoreError> {
    let mut segs: Vec<&CompletedSegment> = completed.iter().collect();
    segs.sort_by_key(|s| s.index);

    let known: std::collections::HashMap<u32, String> = store
        .get_track_segments(session_id, track_id)?
        .into_iter()
        .map(|s| (s.index, s.chain_link))
        .collect();

    let mut prev_link: Option<String> = None;
    for seg in segs {
        let chain_link = if let Some(link) = known.get(&seg.index) {
            link.clone()
        } else {
            let path = resolve_path(dir, &seg.path);
            let sha256 = hash::sha256_file(&path)?;
            let chain_link = hash::chain_link(prev_link.as_deref(), &sha256);
            let size_bytes = std::fs::metadata(&path)?.len();
            store.append_segment(
                session_id,
                &SegmentRecord {
                    track_id,
                    index: seg.index,
                    path: path.to_string_lossy().into_owned(),
                    started_at_unix_ms: seg.started_at_unix_ms,
                    frames: seg.frames,
                    size_bytes,
                    sha256,
                    chain_link: chain_link.clone(),
                },
            )?;
            chain_link
        };
        prev_link = Some(chain_link);
    }
    Ok(prev_link)
}

/// Реконсилировать дорожки многоканальной сессии из `tracks.json` (этап 09):
/// вставить записи дорожек и per-track сегменты из журналов их подкаталогов,
/// зафиксировать финальное звено цепочки каждой дорожки.
fn reconcile_tracks(
    store: &ManifestStore,
    session_id: &str,
    dir: &Path,
    map: &multitrack::TrackMap,
) -> Result<(), StoreError> {
    for entry in &map.tracks {
        store.insert_track(
            session_id,
            &TrackRecord {
                track_id: entry.track_id,
                role: entry.role.clone(),
                label: entry.label.clone(),
                source_device: entry.device.clone(),
                source_channel: entry.channel_index,
                final_chain_link: None,
            },
        )?;
        let track_dir = multitrack::track_dir(dir, entry);
        let journal_path = track_dir.join(journal::JOURNAL_FILE_NAME);
        let completed = if journal_path.exists() {
            journal::replay(&journal_path)?.completed_segments
        } else {
            Vec::new()
        };
        let final_link =
            reconcile_track_segments(store, session_id, entry.track_id, &track_dir, &completed)?;
        if let Some(link) = final_link {
            store.set_track_final_chain_link(session_id, entry.track_id, &link)?;
        }
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
                operator_id: "42".into(),
                station_id: "station-A".into(),
                autonomous_offline: false,
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
                    started_at_unix_ms: 1_700_000_000_000 + (index as u64 - 1) * 30_000,
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
    fn reconcile_persists_real_segment_start_time() {
        // Регрессия этапа 10.1: раньше started_at_unix_ms сегмента жёстко
        // писался как 0 (журнал терял реальный таймкод). Плееру нужен
        // настоящий wall-clock старта сегмента, чтобы сик к метке был точен
        // сквозь паузы записи (offset метки — от wall-clock, не от оси фреймов).
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _files) = build_session_dir(tmp.path(), "sess-1", true);
        let store = ManifestStore::in_memory().unwrap();
        reconcile_session(&store, &dir).unwrap();

        let segs = store.get_segments("sess-1").unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].started_at_unix_ms, 1_700_000_000_000);
        assert_eq!(segs[1].started_at_unix_ms, 1_700_000_030_000);
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
        // Идентичность (этап 10.3) из журнала `SessionStarted` доехала до манифеста.
        assert_eq!(session.operator_id, "42");
        assert_eq!(session.station_id, "station-A");

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

    /// Собрать многоканальную сессию на диске: корневой журнал (start/stop) +
    /// `tracks.json` + подкаталоги дорожек со своими журналами и сегментами.
    fn build_multitrack_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        // Корневой журнал сессии (общий таймкод, старт/стоп).
        let mut journal = Journal::open(&dir).unwrap();
        journal
            .append(&JournalRecord::SessionStarted {
                started_at_unix_ms: 1_700_000_000_000,
                sample_rate_hz: 8_000,
                channels: 2,
                bit_depth: 16,
                segment_seconds: 30,
                operator_id: String::new(),
                station_id: String::new(),
                autonomous_offline: false,
            })
            .unwrap();

        let map = multitrack::track_map_from_resolved(&[
            crate::audio::tracks::ResolvedTrack {
                track_id: 0,
                device: None,
                channel_index: 0,
                role: "judge".into(),
                label: "judge".into(),
            },
            crate::audio::tracks::ResolvedTrack {
                track_id: 1,
                device: None,
                channel_index: 1,
                role: "defense".into(),
                label: "defense".into(),
            },
        ]);
        multitrack::write_track_map(&dir, &map).unwrap();

        // Дорожка judge — 2 сегмента, defense — 1 сегмент; каждая со своим журналом.
        for (entry, n) in map.tracks.iter().zip([2u32, 1u32]) {
            let tdir = multitrack::track_dir(&dir, entry);
            let mut tj = Journal::open(&tdir).unwrap();
            tj.append(&JournalRecord::SessionStarted {
                started_at_unix_ms: 1_700_000_000_000,
                sample_rate_hz: 8_000,
                channels: 1,
                bit_depth: 16,
                segment_seconds: 30,
                operator_id: String::new(),
                station_id: String::new(),
                autonomous_offline: false,
            })
            .unwrap();
            for index in 1..=n {
                let (file_name, frames) = write_segment(&tdir, 100 * index as usize);
                tj.append(&JournalRecord::SegmentCompleted {
                    index,
                    path: file_name,
                    frames,
                    started_at_unix_ms: 1_700_000_000_000 + (index as u64 - 1) * 30_000,
                })
                .unwrap();
            }
            tj.append(&JournalRecord::Stopped).unwrap();
        }
        journal.append(&JournalRecord::Stopped).unwrap();
        dir
    }

    #[test]
    fn reconciles_multitrack_session_into_per_track_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = build_multitrack_dir(tmp.path(), "sess-mt");
        let store = ManifestStore::in_memory().unwrap();

        reconcile_session(&store, &dir).unwrap();

        // Две дорожки с ролями и корректной per-track цепочкой.
        let tracks = store.get_tracks("sess-mt").unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].role, "judge");
        assert_eq!(tracks[1].role, "defense");
        assert_eq!(tracks[1].source_channel, 1);

        let judge = store.get_track_segments("sess-mt", 0).unwrap();
        let defense = store.get_track_segments("sess-mt", 1).unwrap();
        assert_eq!(judge.len(), 2);
        assert_eq!(defense.len(), 1);

        for t in &tracks {
            let segs = store.get_track_segments("sess-mt", t.track_id).unwrap();
            let hashes: Vec<String> = segs.iter().map(|s| s.sha256.clone()).collect();
            let links: Vec<String> = segs.iter().map(|s| s.chain_link.clone()).collect();
            assert!(hash::verify_chain(&hashes, &links, t.final_chain_link.as_deref()));
        }
    }

    #[test]
    fn annotations_survive_restart_via_reconcile() {
        use crate::integrity::annotations::{
            build_annotation_chain, verify_annotation_chain, AnnotationAction, AnnotationRecord,
        };
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _files) = build_session_dir(tmp.path(), "sess-1", false);

        // Оператор ставит разметку в ходе записи (журнал — write-ahead).
        let mut recs = vec![
            AnnotationRecord {
                seq: 1,
                action: AnnotationAction::MarkerAdded,
                target_id: "m1".into(),
                category: Some("Инцидент".into()),
                role: None,
                comment: Some("шум".into()),
                offset_samples: 44_100,
                offset_ms: 1_000,
                operator_id: "op-7".into(),
                at_unix_ms: 1_700_000_001_000,
                chain_link: String::new(),
            },
            AnnotationRecord {
                seq: 2,
                action: AnnotationAction::RoleStarted,
                target_id: "s1".into(),
                category: None,
                role: Some("judge".into()),
                comment: None,
                offset_samples: 88_200,
                offset_ms: 2_000,
                operator_id: "op-7".into(),
                at_unix_ms: 1_700_000_002_000,
                chain_link: String::new(),
            },
        ];
        let chain = build_annotation_chain(&recs);
        for (r, link) in recs.iter_mut().zip(chain) {
            r.chain_link = link;
        }
        let mut journal = Journal::open(&dir).unwrap();
        for r in &recs {
            journal.append(&JournalRecord::Annotation(r.clone())).unwrap();
        }

        // «Рестарт»: чистый манифест, реконсиляция из журнала.
        let store = ManifestStore::in_memory().unwrap();
        reconcile_session(&store, &dir).unwrap();
        let back = store.get_annotations("sess-1").unwrap();
        assert_eq!(back, recs);
        assert!(verify_annotation_chain(&back));

        // Идемпотентно: повторный прогон не плодит дублей.
        reconcile_session(&store, &dir).unwrap();
        assert_eq!(store.get_annotations("sess-1").unwrap().len(), 2);
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

    #[test]
    fn terminal_session_not_rehashed_on_repeat() {
        // Завершённая сессия с тем же числом сегментов при повторной
        // реконсиляции не перехешируется (быстрый показ списка/диагностики).
        let tmp = tempfile::tempdir().unwrap();
        let (dir, files) = build_session_dir(tmp.path(), "sess-1", true);
        let store = ManifestStore::in_memory().unwrap();
        reconcile_session(&store, &dir).unwrap();
        let before = store.get_segments("sess-1").unwrap();

        // Портим файл сегмента на диске: повторный прогон не должен его читать.
        std::fs::write(dir.join(&files[0]), b"tampered bytes on disk").unwrap();
        reconcile_session(&store, &dir).unwrap();
        let after = store.get_segments("sess-1").unwrap();
        assert_eq!(before, after, "хеши терминальной сессии не пересчитываются");
    }

    #[test]
    fn active_session_rehashes_only_new_segments() {
        // Незавершённая (recording) сессия: known-сегменты не перехешируются,
        // новый сегмент — добавляется с пересчётом цепочки.
        let tmp = tempfile::tempdir().unwrap();
        let (dir, files) = build_session_dir(tmp.path(), "sess-1", false);
        let store = ManifestStore::in_memory().unwrap();
        reconcile_session(&store, &dir).unwrap();
        let seg1_before = store.get_segments("sess-1").unwrap()[0].clone();

        // Портим уже известный сегмент 1 и дописываем новый сегмент 3.
        std::fs::write(dir.join(&files[0]), b"tampered bytes on disk").unwrap();
        let (name3, frames3) = write_segment(&dir, 300);
        let mut journal = Journal::open(&dir).unwrap();
        journal
            .append(&JournalRecord::SegmentCompleted {
                index: 3,
                path: name3,
                frames: frames3,
                started_at_unix_ms: 1_700_000_060_000,
            })
            .unwrap();

        reconcile_session(&store, &dir).unwrap();
        let segs = store.get_segments("sess-1").unwrap();
        // Сегмент 1 не перехеширован (порча файла не повлияла на манифест)…
        assert_eq!(segs[0], seg1_before);
        // …а новый сегмент 3 добавлен.
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[2].index, 3);
    }
}
