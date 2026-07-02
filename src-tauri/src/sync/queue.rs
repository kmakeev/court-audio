//! Персистентная оффлайн-очередь выгрузки (этап 06 — `promts/06_sync_agent.md`,
//! шаг 2).
//!
//! Очередь **записей** — производная от манифеста ([`uploadable`]): запись «в
//! очереди», если она завершена (`stopped`/`recovered`), ещё не подтверждена
//! (`upload_status ∈ {pending, uploading, failed}`), не на паузе и локальная
//! копия на месте. Это естественно переживает рестарт приложения.
//!
//! Под-прогресс выгрузки — в таблицах `upload_state` (серверный `recording_id`
//! и факт `init`) и `upload_parts` (по-сегментная докачка, «часть = сегмент»):
//! трекинг отправленных частей делает повтор идемпотентным, а возобновление —
//! продолжением с неотправленных. Все операции — поверх соединения
//! [`ManifestStore::conn`]; на горячем пути аудио не вызываются.

use rusqlite::OptionalExtension;

use crate::store::export::RecordingManifest;
use crate::store::manifest::{ManifestStore, SessionRecord, SessionStatus, UploadStatus};
use crate::store::StoreError;

/// Состояние части выгрузки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartState {
    /// Ещё не отправлена (или отправка не подтверждена).
    Pending,
    /// Принята сервером.
    Sent,
}

impl PartState {
    fn as_code(self) -> &'static str {
        match self {
            PartState::Pending => "pending",
            PartState::Sent => "sent",
        }
    }
}

/// Часть выгрузки (= сегмент дорожки) с трекингом докачки. Часть адресуется по
/// `(track_id, part_index)` — многоканал (этап 09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartRow {
    pub track_id: u32,
    pub part_index: u32,
    pub size_bytes: u64,
    pub sha256: String,
    pub state: PartState,
    pub attempts: u32,
}

/// Прогресс выгрузки записи (для статуса в UI «выгружается N%»).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartProgress {
    pub total: u32,
    pub sent: u32,
}

/// Записи, подлежащие выгрузке на момент опроса (новые сверху, как в манифесте).
/// Фильтр: завершена, не подтверждена, не на паузе, локальная копия не удалена.
pub fn uploadable(store: &ManifestStore) -> Result<Vec<SessionRecord>, StoreError> {
    Ok(store
        .list_sessions()?
        .into_iter()
        .filter(is_uploadable)
        .collect())
}

/// Подлежит ли запись автоматической выгрузке.
pub fn is_uploadable(s: &SessionRecord) -> bool {
    let finished = matches!(s.status, SessionStatus::Stopped | SessionStatus::Recovered);
    let needs_upload = matches!(
        s.upload_status,
        UploadStatus::Pending | UploadStatus::Uploading | UploadStatus::Failed
    );
    finished && needs_upload && !s.upload_paused && s.local_purged_at_unix_ms.is_none()
}

/// Завести/обновить трекинг частей из манифеста записи (идемпотентно): создаёт
/// строку `upload_state` и part-строки в статусе `pending`. Повторный вызов
/// **не сбрасывает** уже отправленные части (`INSERT OR IGNORE`) — безопасен при
/// возобновлении.
pub fn init_parts_from_manifest(
    store: &ManifestStore,
    session_id: &str,
    manifest: &RecordingManifest,
) -> Result<(), StoreError> {
    let conn = store.conn();
    conn.execute(
        "INSERT OR IGNORE INTO upload_state (session_id, server_recording_id, init_done)
         VALUES (?1, NULL, 0)",
        rusqlite::params![session_id],
    )?;
    for track in &manifest.tracks {
        for seg in &track.segments {
            conn.execute(
                "INSERT OR IGNORE INTO upload_parts
                    (session_id, track_id, part_index, size_bytes, sha256, state, attempts, last_error)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 0, NULL)",
                rusqlite::params![
                    session_id,
                    track.track_id,
                    seg.index,
                    seg.size_bytes as i64,
                    seg.sha256
                ],
            )?;
        }
    }
    Ok(())
}

/// Части записи по всем дорожкам в порядке `(track_id, part_index)`.
pub fn all_parts(store: &ManifestStore, session_id: &str) -> Result<Vec<PartRow>, StoreError> {
    let conn = store.conn();
    let mut stmt = conn.prepare(
        "SELECT track_id, part_index, size_bytes, sha256, state, attempts
         FROM upload_parts WHERE session_id = ?1 ORDER BY track_id ASC, part_index ASC",
    )?;
    let rows = stmt.query_map([session_id], |row| {
        let state_code: String = row.get(4)?;
        Ok((
            row.get::<_, i64>(0)? as u32,
            row.get::<_, i64>(1)? as u32,
            row.get::<_, i64>(2)? as u64,
            row.get::<_, String>(3)?,
            state_code,
            row.get::<_, i64>(5)? as u32,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        let (track_id, part_index, size_bytes, sha256, state_code, attempts) = r?;
        let state = match state_code.as_str() {
            "sent" => PartState::Sent,
            _ => PartState::Pending,
        };
        out.push(PartRow {
            track_id,
            part_index,
            size_bytes,
            sha256,
            state,
            attempts,
        });
    }
    Ok(out)
}

/// Неотправленные части (для докачки): продолжение с того, что не принято.
pub fn pending_parts(store: &ManifestStore, session_id: &str) -> Result<Vec<PartRow>, StoreError> {
    Ok(all_parts(store, session_id)?
        .into_iter()
        .filter(|p| p.state == PartState::Pending)
        .collect())
}

/// Прогресс выгрузки записи (всего/отправлено) — для статуса в UI.
pub fn progress(store: &ManifestStore, session_id: &str) -> Result<PartProgress, StoreError> {
    let parts = all_parts(store, session_id)?;
    let total = parts.len() as u32;
    let sent = parts.iter().filter(|p| p.state == PartState::Sent).count() as u32;
    Ok(PartProgress { total, sent })
}

/// Отметить часть дорожки принятой сервером.
pub fn mark_part_sent(
    store: &ManifestStore,
    session_id: &str,
    track_id: u32,
    part_index: u32,
) -> Result<(), StoreError> {
    set_part_state(store, session_id, track_id, part_index, PartState::Sent, None)
}

/// Зафиксировать неуспешную попытку части (инкремент `attempts` + текст ошибки),
/// оставив её `pending` для следующего прохода.
pub fn record_part_attempt(
    store: &ManifestStore,
    session_id: &str,
    track_id: u32,
    part_index: u32,
    error: &str,
) -> Result<(), StoreError> {
    store.conn().execute(
        "UPDATE upload_parts SET attempts = attempts + 1, last_error = ?4
         WHERE session_id = ?1 AND track_id = ?2 AND part_index = ?3",
        rusqlite::params![session_id, track_id, part_index, error],
    )?;
    Ok(())
}

fn set_part_state(
    store: &ManifestStore,
    session_id: &str,
    track_id: u32,
    part_index: u32,
    state: PartState,
    error: Option<&str>,
) -> Result<(), StoreError> {
    store.conn().execute(
        "UPDATE upload_parts SET state = ?4, last_error = ?5
         WHERE session_id = ?1 AND track_id = ?2 AND part_index = ?3",
        rusqlite::params![session_id, track_id, part_index, state.as_code(), error],
    )?;
    Ok(())
}

/// Серверный `recording_id` записи (если уже зарегистрирована).
pub fn get_recording_id(
    store: &ManifestStore,
    session_id: &str,
) -> Result<Option<String>, StoreError> {
    Ok(store
        .conn()
        .query_row(
            "SELECT server_recording_id FROM upload_state WHERE session_id = ?1",
            [session_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// Сохранить серверный `recording_id` (после регистрации сессии).
pub fn set_recording_id(
    store: &ManifestStore,
    session_id: &str,
    recording_id: &str,
) -> Result<(), StoreError> {
    store.conn().execute(
        "INSERT INTO upload_state (session_id, server_recording_id, init_done)
         VALUES (?1, ?2, 0)
         ON CONFLICT(session_id) DO UPDATE SET server_recording_id = excluded.server_recording_id",
        rusqlite::params![session_id, recording_id],
    )?;
    Ok(())
}

/// Сделан ли серверный `upload/init` (заявка состава сегментов).
pub fn is_init_done(store: &ManifestStore, session_id: &str) -> Result<bool, StoreError> {
    Ok(store
        .conn()
        .query_row(
            "SELECT init_done FROM upload_state WHERE session_id = ?1",
            [session_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .map(|v| v != 0)
        .unwrap_or(false))
}

/// Отметить `upload/init` выполненным.
pub fn mark_init_done(store: &ManifestStore, session_id: &str) -> Result<(), StoreError> {
    store.conn().execute(
        "UPDATE upload_state SET init_done = 1 WHERE session_id = ?1",
        [session_id],
    )?;
    Ok(())
}

/// Сбросить части в `pending` для ручного повтора оператором (статус ошибки →
/// заново). Не трогает уже принятые сервером части — докачиваем только остаток.
pub fn reset_for_retry(store: &ManifestStore, session_id: &str) -> Result<(), StoreError> {
    store.conn().execute(
        "UPDATE upload_parts SET last_error = NULL
         WHERE session_id = ?1 AND state = 'pending'",
        [session_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::export::{
        AnnotationsExport, IntegrityMeta, RecordingManifest, SegmentEntry, SessionMeta, TrackEntry,
    };
    use crate::store::manifest::SessionStatus;

    /// Манифест с одной дорожкой (`single`) на `n` сегментов.
    fn manifest_with_segments(n: u32) -> RecordingManifest {
        manifest_with_tracks(&[(0, "single", n)])
    }

    /// Манифест с заданными дорожками `(track_id, role, кол-во сегментов)`.
    fn manifest_with_tracks(spec: &[(u32, &str, u32)]) -> RecordingManifest {
        let tracks = spec
            .iter()
            .map(|(tid, role, n)| TrackEntry {
                track_id: *tid,
                role: role.to_string(),
                label: role.to_string(),
                final_chain_link: Some(format!("final{tid}")),
                segments: (1..=*n)
                    .map(|i| SegmentEntry {
                        track_id: *tid,
                        index: i,
                        sha256: format!("hash{tid}-{i}"),
                        chain_link: format!("link{tid}-{i}"),
                        size_bytes: 1000 * i as u64,
                        frames: 100 * i as u64,
                        started_at_unix_ms: i as u64,
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        let track_count = tracks.len() as u32;
        RecordingManifest {
            manifest_version: 3,
            session: SessionMeta {
                id: "s1".into(),
                started_at_unix_ms: 1,
                status: SessionStatus::Stopped,
                station_id: "st".into(),
                operator_id: "op".into(),
                adjudication_ref: None,
                sample_rate_hz: 44_100,
                channels: 1,
                bit_depth: 16,
                track_count,
                upload_status: UploadStatus::Pending,
                final_chain_link: Some("link".into()),
            },
            integrity: IntegrityMeta {
                segment_hash: "sha256".into(),
                hash_chain: true,
            },
            tracks,
            events: vec![],
            annotations: AnnotationsExport::default(),
        }
    }

    fn seed_session(store: &ManifestStore, id: &str) {
        store
            .insert_session(&SessionRecord::new(
                id,
                format!("/rec/{id}"),
                1,
                "st",
                "op",
                44_100,
                1,
                16,
            ))
            .unwrap();
        store.set_status(id, SessionStatus::Stopped).unwrap();
    }

    #[test]
    fn init_parts_is_idempotent_and_keeps_sent() {
        let store = ManifestStore::in_memory().unwrap();
        seed_session(&store, "s1");
        let m = manifest_with_segments(3);
        init_parts_from_manifest(&store, "s1", &m).unwrap();
        assert_eq!(pending_parts(&store, "s1").unwrap().len(), 3);

        // Отправили часть 1 (дорожка 0).
        mark_part_sent(&store, "s1", 0, 1).unwrap();
        // Повторный init не сбрасывает отправленную часть.
        init_parts_from_manifest(&store, "s1", &m).unwrap();
        let pending = pending_parts(&store, "s1").unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].part_index, 2);
        assert_eq!(
            progress(&store, "s1").unwrap(),
            PartProgress { total: 3, sent: 1 }
        );
    }

    #[test]
    fn multitrack_parts_are_tracked_per_track() {
        let store = ManifestStore::in_memory().unwrap();
        seed_session(&store, "s1");
        // Две дорожки: judge (2 части) + defense (1 часть) = 3 части всего.
        let m = manifest_with_tracks(&[(0, "judge", 2), (1, "defense", 1)]);
        init_parts_from_manifest(&store, "s1", &m).unwrap();
        assert_eq!(all_parts(&store, "s1").unwrap().len(), 3);

        // Часть 1 дорожки judge и часть 1 дорожки defense — разные части.
        mark_part_sent(&store, "s1", 0, 1).unwrap();
        let pending = pending_parts(&store, "s1").unwrap();
        assert_eq!(pending.len(), 2);
        // Часть (0,1) отправлена, (0,2) и (1,1) — ещё нет.
        assert!(pending
            .iter()
            .any(|p| p.track_id == 1 && p.part_index == 1));
        assert!(pending
            .iter()
            .any(|p| p.track_id == 0 && p.part_index == 2));
        assert_eq!(
            progress(&store, "s1").unwrap(),
            PartProgress { total: 3, sent: 1 }
        );
    }

    #[test]
    fn recording_id_and_init_flag_persist() {
        let store = ManifestStore::in_memory().unwrap();
        seed_session(&store, "s1");
        init_parts_from_manifest(&store, "s1", &manifest_with_segments(1)).unwrap();
        assert!(get_recording_id(&store, "s1").unwrap().is_none());
        set_recording_id(&store, "s1", "rec-42").unwrap();
        assert_eq!(
            get_recording_id(&store, "s1").unwrap().as_deref(),
            Some("rec-42")
        );
        assert!(!is_init_done(&store, "s1").unwrap());
        mark_init_done(&store, "s1").unwrap();
        assert!(is_init_done(&store, "s1").unwrap());
    }

    #[test]
    fn record_attempt_increments_and_keeps_pending() {
        let store = ManifestStore::in_memory().unwrap();
        seed_session(&store, "s1");
        init_parts_from_manifest(&store, "s1", &manifest_with_segments(1)).unwrap();
        record_part_attempt(&store, "s1", 0, 1, "нет сети").unwrap();
        record_part_attempt(&store, "s1", 0, 1, "нет сети").unwrap();
        let parts = all_parts(&store, "s1").unwrap();
        assert_eq!(parts[0].attempts, 2);
        assert_eq!(parts[0].state, PartState::Pending);
    }

    #[test]
    fn uploadable_filters_by_status_and_pause() {
        let store = ManifestStore::in_memory().unwrap();
        // Завершена + pending → в очереди.
        seed_session(&store, "ready");
        // Ещё пишется → не в очереди.
        store
            .insert_session(&SessionRecord::new(
                "rec", "/rec/rec", 1, "st", "op", 44_100, 1, 16,
            ))
            .unwrap();
        // Подтверждена → не в очереди.
        seed_session(&store, "done");
        store
            .set_upload_status("done", UploadStatus::Confirmed)
            .unwrap();
        // На паузе → не в очереди.
        seed_session(&store, "paused");
        store.set_upload_paused("paused", true).unwrap();

        let ids: Vec<String> = uploadable(&store)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec!["ready".to_string()]);
    }
}
