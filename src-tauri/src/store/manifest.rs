//! Манифест сессии: модель и запись по ходу записи (этап 03 —
//! `promts/03_store_integrity.md`, шаг 2).
//!
//! [`ManifestStore`] поверх SQLite ([`super::db`]) — запросы для UI/экспорта/
//! ретеншна и обновления по мере финализации сегментов. **Не на горячем пути
//! аудио:** вызывается потребителем при `drain_completed` (как журнал этапа 02),
//! а live-захват от SQLite не зависит. Хеши/цепочку считает [`crate::integrity`],
//! шифрование — [`super::crypto`]; здесь — только модель и персист.

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::db;
use super::StoreError;
use crate::integrity::events::{EventKind, RecordingEvent};

/// Статус сессии в манифесте.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Идёт запись.
    Recording,
    /// Завершена штатно.
    Stopped,
    /// Восстановлена после сбоя (продолжена/закрыта).
    Recovered,
    /// Локальная копия удалена ретеншном (tombstone для истории UI).
    Purged,
}

impl SessionStatus {
    pub fn as_code(self) -> &'static str {
        match self {
            SessionStatus::Recording => "recording",
            SessionStatus::Stopped => "stopped",
            SessionStatus::Recovered => "recovered",
            SessionStatus::Purged => "purged",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "recording" => Some(SessionStatus::Recording),
            "stopped" => Some(SessionStatus::Stopped),
            "recovered" => Some(SessionStatus::Recovered),
            "purged" => Some(SessionStatus::Purged),
            _ => None,
        }
    }
}

/// Статус выгрузки записи в `ex_system` (наполняется этапом `06`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadStatus {
    /// Ещё не выгружалась.
    Pending,
    /// Идёт выгрузка.
    Uploading,
    /// Выгружена; ожидает серверного подтверждения целостности.
    Uploaded,
    /// Сервер подтвердил приём и целостность.
    Confirmed,
    /// Выгрузка завершилась временной ошибкой (сеть/5xx) — будет ретрай (`06`).
    Failed,
    /// Сервер не подтвердил целостность (`verify=false`, подмена сегмента):
    /// терминально, локальную копию **не** удаляем (`06`).
    IntegrityFailed,
}

impl UploadStatus {
    pub fn as_code(self) -> &'static str {
        match self {
            UploadStatus::Pending => "pending",
            UploadStatus::Uploading => "uploading",
            UploadStatus::Uploaded => "uploaded",
            UploadStatus::Confirmed => "confirmed",
            UploadStatus::Failed => "failed",
            UploadStatus::IntegrityFailed => "integrity_failed",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "pending" => Some(UploadStatus::Pending),
            "uploading" => Some(UploadStatus::Uploading),
            "uploaded" => Some(UploadStatus::Uploaded),
            "confirmed" => Some(UploadStatus::Confirmed),
            "failed" => Some(UploadStatus::Failed),
            "integrity_failed" => Some(UploadStatus::IntegrityFailed),
            _ => None,
        }
    }
}

/// Запись сессии в манифесте.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Идентификатор сессии (= имя каталога сессии).
    pub id: String,
    /// Каталог сессии (журнал + сегменты).
    pub dir: String,
    pub started_at_unix_ms: u64,
    pub status: SessionStatus,
    pub station_id: String,
    pub operator_id: String,
    /// Привязка к делу (`Adjudication`) — наполняется этапом `05`.
    pub adjudication_ref: Option<String>,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub bit_depth: u16,
    /// Финальное звено хеш-цепочки сессии (итог целостности).
    pub final_chain_link: Option<String>,
    pub upload_status: UploadStatus,
    /// Сервер подтвердил целостность (`07`); триггер ретеншна (`06`).
    pub server_integrity_verified: bool,
    pub confirmed_at_unix_ms: Option<u64>,
    pub local_purged_at_unix_ms: Option<u64>,
    /// Операторская пауза догрузки (`06`): планировщик пропускает такие сессии.
    pub upload_paused: bool,
}

impl SessionRecord {
    /// Новая активная сессия (статус `Recording`, ещё не выгружалась).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        dir: impl Into<String>,
        started_at_unix_ms: u64,
        station_id: impl Into<String>,
        operator_id: impl Into<String>,
        sample_rate_hz: u32,
        channels: u16,
        bit_depth: u16,
    ) -> Self {
        Self {
            id: id.into(),
            dir: dir.into(),
            started_at_unix_ms,
            status: SessionStatus::Recording,
            station_id: station_id.into(),
            operator_id: operator_id.into(),
            adjudication_ref: None,
            sample_rate_hz,
            channels,
            bit_depth,
            final_chain_link: None,
            upload_status: UploadStatus::Pending,
            server_integrity_verified: false,
            confirmed_at_unix_ms: None,
            local_purged_at_unix_ms: None,
            upload_paused: false,
        }
    }
}

/// Запись сегмента в манифесте.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentRecord {
    /// Дорожка сегмента (многоканал, этап 09). `0` — единственная дорожка v1.
    #[serde(default)]
    pub track_id: u32,
    pub index: u32,
    /// Путь к файлу сегмента на диске (открытый WAV или `.enc` при шифровании).
    pub path: String,
    pub started_at_unix_ms: u64,
    /// Длительность в кадрах (семплов на канал).
    pub frames: u64,
    /// Размер каноничного контента (WAV до шифрования), по которому считан хеш.
    pub size_bytes: u64,
    /// SHA-256 каноничного контента (hex).
    pub sha256: String,
    /// Звено хеш-цепочки на этом сегменте (в рамках своей дорожки).
    pub chain_link: String,
}

/// Дорожка сессии в карте «канал ↔ роль» (многоканал, этап 09). Легаси-сессии
/// (моно) имеют одну дорожку `track_id = 0` роль `single`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackRecord {
    pub track_id: u32,
    /// Роль дорожки (из `audio.roles`; для легаси — `single`).
    pub role: String,
    pub label: String,
    /// Устройство-источник (`None` — системное по умолчанию).
    pub source_device: Option<String>,
    /// Индекс канала в интерливнутом потоке устройства.
    pub source_channel: u16,
    /// Финальное звено хеш-цепочки **этой дорожки** (per-track целостность).
    pub final_chain_link: Option<String>,
}

/// Запись события в манифесте (событие + порядковый номер).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub seq: i64,
    #[serde(flatten)]
    pub event: RecordingEvent,
}

/// Манифест поверх соединения SQLite. Владеет соединением.
pub struct ManifestStore {
    conn: Connection,
}

impl ManifestStore {
    /// Открыть манифест по пути (создаёт/мигрирует схему).
    pub fn open(path: &std::path::Path) -> Result<Self, StoreError> {
        Ok(Self {
            conn: db::open(path)?,
        })
    }

    /// Манифест в памяти (тесты).
    pub fn in_memory() -> Result<Self, StoreError> {
        Ok(Self {
            conn: db::open_in_memory()?,
        })
    }

    /// Доступ к соединению (для смежных подсистем store).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Вставить сессию (idempotent upsert по `id`).
    pub fn insert_session(&self, s: &SessionRecord) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO sessions (
                id, dir, started_at_unix_ms, status, station_id, operator_id,
                adjudication_ref, sample_rate_hz, channels, bit_depth,
                final_chain_link, upload_status, server_integrity_verified,
                confirmed_at_unix_ms, local_purged_at_unix_ms, upload_paused
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
            ON CONFLICT(id) DO UPDATE SET
                dir=excluded.dir,
                started_at_unix_ms=excluded.started_at_unix_ms,
                status=excluded.status,
                station_id=excluded.station_id,
                operator_id=excluded.operator_id,
                adjudication_ref=excluded.adjudication_ref,
                sample_rate_hz=excluded.sample_rate_hz,
                channels=excluded.channels,
                bit_depth=excluded.bit_depth,
                final_chain_link=excluded.final_chain_link,
                upload_status=excluded.upload_status,
                server_integrity_verified=excluded.server_integrity_verified,
                confirmed_at_unix_ms=excluded.confirmed_at_unix_ms,
                local_purged_at_unix_ms=excluded.local_purged_at_unix_ms,
                upload_paused=excluded.upload_paused",
            rusqlite::params![
                s.id,
                s.dir,
                s.started_at_unix_ms as i64,
                s.status.as_code(),
                s.station_id,
                s.operator_id,
                s.adjudication_ref,
                s.sample_rate_hz as i64,
                s.channels as i64,
                s.bit_depth as i64,
                s.final_chain_link,
                s.upload_status.as_code(),
                s.server_integrity_verified as i64,
                s.confirmed_at_unix_ms.map(|v| v as i64),
                s.local_purged_at_unix_ms.map(|v| v as i64),
                s.upload_paused as i64,
            ],
        )?;
        Ok(())
    }

    /// Добавить/обновить сегмент сессии (upsert по `(session_id, track_id, idx)`).
    /// `track_id` берётся из самой записи (`0` — единственная дорожка v1).
    pub fn append_segment(&self, session_id: &str, seg: &SegmentRecord) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO segments (
                session_id, track_id, idx, path, started_at_unix_ms, frames, size_bytes, sha256, chain_link
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
            ON CONFLICT(session_id, track_id, idx) DO UPDATE SET
                path=excluded.path,
                started_at_unix_ms=excluded.started_at_unix_ms,
                frames=excluded.frames,
                size_bytes=excluded.size_bytes,
                sha256=excluded.sha256,
                chain_link=excluded.chain_link",
            rusqlite::params![
                session_id,
                seg.track_id as i64,
                seg.index as i64,
                seg.path,
                seg.started_at_unix_ms as i64,
                seg.frames as i64,
                seg.size_bytes as i64,
                seg.sha256,
                seg.chain_link,
            ],
        )?;
        Ok(())
    }

    /// Вставить/обновить дорожку сессии (upsert по `(session_id, track_id)`).
    pub fn insert_track(&self, session_id: &str, t: &TrackRecord) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO tracks (
                session_id, track_id, role, label, source_device, source_channel, final_chain_link
            ) VALUES (?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(session_id, track_id) DO UPDATE SET
                role=excluded.role,
                label=excluded.label,
                source_device=excluded.source_device,
                source_channel=excluded.source_channel,
                final_chain_link=excluded.final_chain_link",
            rusqlite::params![
                session_id,
                t.track_id as i64,
                t.role,
                t.label,
                t.source_device,
                t.source_channel as i64,
                t.final_chain_link,
            ],
        )?;
        Ok(())
    }

    /// Зафиксировать финальное звено хеш-цепочки дорожки (per-track целостность).
    pub fn set_track_final_chain_link(
        &self,
        session_id: &str,
        track_id: u32,
        link: &str,
    ) -> Result<(), StoreError> {
        let n = self.conn.execute(
            "UPDATE tracks SET final_chain_link = ?3 WHERE session_id = ?1 AND track_id = ?2",
            rusqlite::params![session_id, track_id as i64, link],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!(
                "дорожка {track_id} сессии {session_id}"
            )));
        }
        Ok(())
    }

    /// Дорожки сессии в порядке `track_id`.
    pub fn get_tracks(&self, session_id: &str) -> Result<Vec<TrackRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT track_id, role, label, source_device, source_channel, final_chain_link
             FROM tracks WHERE session_id = ?1 ORDER BY track_id ASC",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok(TrackRecord {
                track_id: row.get::<_, i64>(0)? as u32,
                role: row.get(1)?,
                label: row.get(2)?,
                source_device: row.get(3)?,
                source_channel: row.get::<_, i64>(4)? as u16,
                final_chain_link: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Дорожки сессии с учётом легаси-совместимости: если явных дорожек нет
    /// (сессия, наполненная до многоканала, либо тесты без явных дорожек),
    /// синтезируется одна дорожка `track_id = 0`, роль [`SINGLE_TRACK_ROLE`].
    /// Общий помощник для `store::export::build_manifest`,
    /// `ipc::player_cmds::player_open_session` и `export::package` — вместо
    /// дублирования одной и той же развилки в трёх местах.
    pub fn resolve_tracks(&self, session_id: &str) -> Result<Vec<TrackRecord>, StoreError> {
        let tracks = self.get_tracks(session_id)?;
        if !tracks.is_empty() {
            return Ok(tracks);
        }
        let session = self
            .get_session(session_id)?
            .ok_or_else(|| StoreError::NotFound(format!("сессия {session_id}")))?;
        Ok(vec![TrackRecord {
            track_id: 0,
            role: crate::audio::tracks::SINGLE_TRACK_ROLE.to_string(),
            label: String::new(),
            source_device: None,
            source_channel: 0,
            final_chain_link: session.final_chain_link,
        }])
    }

    /// Сегменты одной дорожки в порядке индекса.
    pub fn get_track_segments(
        &self,
        session_id: &str,
        track_id: u32,
    ) -> Result<Vec<SegmentRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT track_id, idx, path, started_at_unix_ms, frames, size_bytes, sha256, chain_link
             FROM segments WHERE session_id = ?1 AND track_id = ?2 ORDER BY idx ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, track_id as i64], map_segment)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Дописать событие; возвращает присвоенный `seq` (монотонный в рамках сессии).
    pub fn append_event(
        &self,
        session_id: &str,
        event: &RecordingEvent,
    ) -> Result<i64, StoreError> {
        let next_seq: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        )?;
        let detail_json = match &event.detail {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        self.conn.execute(
            "INSERT INTO events (session_id, seq, kind, at_unix_ms, detail_json)
             VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![
                session_id,
                next_seq,
                event.kind.as_code(),
                event.at_unix_ms as i64,
                detail_json,
            ],
        )?;
        Ok(next_seq)
    }

    /// Дописать событие изменения настроек в станционный журнал (этап 10.4);
    /// возвращает присвоенный автоинкрементный `seq`. Не привязано к сессии.
    pub fn append_settings_change(
        &self,
        change: &super::settings_audit::SettingsChange,
    ) -> Result<i64, StoreError> {
        let changes_json = serde_json::to_string(&change.changes)?;
        self.conn.execute(
            "INSERT INTO settings_audit
                (at_unix_ms, actor_operator_id, source, dangerous, changes_json)
             VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![
                change.at_unix_ms as i64,
                change.actor_operator_id,
                change.source.as_code(),
                change.dangerous as i64,
                changes_json,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Прочитать журнал изменений настроек (новейшие сверху, не более `limit`).
    pub fn list_settings_audit(
        &self,
        limit: u32,
    ) -> Result<Vec<super::settings_audit::SettingsAuditRecord>, StoreError> {
        use super::settings_audit::{ChangeSource, FieldChange, SettingsAuditRecord, SettingsChange};
        let mut stmt = self.conn.prepare(
            "SELECT seq, at_unix_ms, actor_operator_id, source, dangerous, changes_json
             FROM settings_audit ORDER BY seq DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            let seq: i64 = row.get(0)?;
            let at_unix_ms: i64 = row.get(1)?;
            let actor_operator_id: String = row.get(2)?;
            let source_code: String = row.get(3)?;
            let dangerous: i64 = row.get(4)?;
            let changes_json: String = row.get(5)?;
            Ok((seq, at_unix_ms, actor_operator_id, source_code, dangerous, changes_json))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (seq, at, actor, source_code, dangerous, changes_json) = r?;
            let source = ChangeSource::from_code(&source_code)
                .ok_or_else(|| StoreError::Db(format!("неизвестный source журнала: {source_code}")))?;
            let changes: Vec<FieldChange> = serde_json::from_str(&changes_json)?;
            out.push(SettingsAuditRecord {
                seq,
                change: SettingsChange {
                    at_unix_ms: at as u64,
                    actor_operator_id: actor,
                    source,
                    dangerous: dangerous != 0,
                    changes,
                },
            });
        }
        Ok(out)
    }

    /// Зафиксировать финальное звено хеш-цепочки сессии.
    pub fn set_final_chain_link(&self, session_id: &str, link: &str) -> Result<(), StoreError> {
        self.update_session_field(
            session_id,
            "final_chain_link = ?2",
            rusqlite::params![session_id, link],
        )
    }

    /// Изменить статус сессии.
    pub fn set_status(&self, session_id: &str, status: SessionStatus) -> Result<(), StoreError> {
        self.update_session_field(
            session_id,
            "status = ?2",
            rusqlite::params![session_id, status.as_code()],
        )
    }

    /// Изменить статус выгрузки (наполняется `06`).
    pub fn set_upload_status(
        &self,
        session_id: &str,
        status: UploadStatus,
    ) -> Result<(), StoreError> {
        self.update_session_field(
            session_id,
            "upload_status = ?2",
            rusqlite::params![session_id, status.as_code()],
        )
    }

    /// Поставить/снять операторскую паузу догрузки (наполняется `06`).
    pub fn set_upload_paused(&self, session_id: &str, paused: bool) -> Result<(), StoreError> {
        self.update_session_field(
            session_id,
            "upload_paused = ?2",
            rusqlite::params![session_id, paused as i64],
        )
    }

    /// Привязать сессию к делу (наполняется `05`).
    pub fn set_adjudication_ref(
        &self,
        session_id: &str,
        adjudication_ref: Option<&str>,
    ) -> Result<(), StoreError> {
        self.update_session_field(
            session_id,
            "adjudication_ref = ?2",
            rusqlite::params![session_id, adjudication_ref],
        )
    }

    /// Триггер подтверждения сервером (вызывается из `06` через
    /// [`super::retention::mark_server_confirmed`]): фиксирует флаг целостности,
    /// момент подтверждения и статус выгрузки.
    pub fn mark_server_confirmed(
        &self,
        session_id: &str,
        integrity_verified: bool,
        at_unix_ms: u64,
    ) -> Result<(), StoreError> {
        let upload = if integrity_verified {
            UploadStatus::Confirmed
        } else {
            UploadStatus::Uploaded
        };
        self.update_session_field(
            session_id,
            "server_integrity_verified = ?2, confirmed_at_unix_ms = ?3, upload_status = ?4",
            rusqlite::params![
                session_id,
                integrity_verified as i64,
                at_unix_ms as i64,
                upload.as_code()
            ],
        )
    }

    /// Пометить локальную копию удалённой ретеншном (tombstone).
    pub fn mark_purged(&self, session_id: &str, at_unix_ms: u64) -> Result<(), StoreError> {
        self.update_session_field(
            session_id,
            "status = ?2, local_purged_at_unix_ms = ?3",
            rusqlite::params![
                session_id,
                SessionStatus::Purged.as_code(),
                at_unix_ms as i64
            ],
        )
    }

    fn update_session_field(
        &self,
        session_id: &str,
        set_clause: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<(), StoreError> {
        let sql = format!("UPDATE sessions SET {set_clause} WHERE id = ?1");
        let n = self.conn.execute(&sql, params)?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("сессия {session_id}")));
        }
        Ok(())
    }

    /// Прочитать сессию по `id`.
    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>, StoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT id, dir, started_at_unix_ms, status, station_id, operator_id,
                        adjudication_ref, sample_rate_hz, channels, bit_depth,
                        final_chain_link, upload_status, server_integrity_verified,
                        confirmed_at_unix_ms, local_purged_at_unix_ms, upload_paused
                 FROM sessions WHERE id = ?1",
                [session_id],
                map_session,
            )
            .optional()?;
        row.transpose()
    }

    /// Все сессии (для UI/ретеншна), новые сверху.
    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, dir, started_at_unix_ms, status, station_id, operator_id,
                    adjudication_ref, sample_rate_hz, channels, bit_depth,
                    final_chain_link, upload_status, server_integrity_verified,
                    confirmed_at_unix_ms, local_purged_at_unix_ms, upload_paused
             FROM sessions ORDER BY started_at_unix_ms DESC",
        )?;
        let rows = stmt.query_map([], map_session)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// Все сегменты сессии по всем дорожкам, порядок `(track_id, idx)`. Для
    /// одноканальной сессии (единственная дорожка `0`) — это порядок по `idx`.
    pub fn get_segments(&self, session_id: &str) -> Result<Vec<SegmentRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT track_id, idx, path, started_at_unix_ms, frames, size_bytes, sha256, chain_link
             FROM segments WHERE session_id = ?1 ORDER BY track_id ASC, idx ASC",
        )?;
        let rows = stmt.query_map([session_id], map_segment)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// События сессии в порядке `seq`.
    pub fn get_events(&self, session_id: &str) -> Result<Vec<EventRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, kind, at_unix_ms, detail_json
             FROM events WHERE session_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            let seq: i64 = row.get(0)?;
            let kind_code: String = row.get(1)?;
            let at_unix_ms: i64 = row.get(2)?;
            let detail_json: Option<String> = row.get(3)?;
            Ok((seq, kind_code, at_unix_ms as u64, detail_json))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (seq, kind_code, at_unix_ms, detail_json) = r?;
            let kind = EventKind::from_code(&kind_code).ok_or_else(|| {
                StoreError::Serde(format!("неизвестный kind события: {kind_code}"))
            })?;
            let detail = match detail_json {
                Some(s) => Some(serde_json::from_str(&s)?),
                None => None,
            };
            out.push(EventRecord {
                seq,
                event: RecordingEvent {
                    kind,
                    at_unix_ms,
                    detail,
                },
            });
        }
        Ok(out)
    }

    /// Удалить сегменты и события сессии (используется ретеншном после удаления
    /// файлов). Сама строка сессии остаётся tombstone'ом (см. [`super::retention`]).
    pub fn delete_segments_and_events(&self, session_id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM segments WHERE session_id = ?1", [session_id])?;
        self.conn
            .execute("DELETE FROM events WHERE session_id = ?1", [session_id])?;
        // Разметка (этап 10) — тоже локальная копия, удаляется при purge.
        self.conn
            .execute("DELETE FROM annotations WHERE session_id = ?1", [session_id])?;
        Ok(())
    }
}

/// Маппинг строки segments (колонки: track_id, idx, path, started_at_unix_ms,
/// frames, size_bytes, sha256, chain_link) в [`SegmentRecord`].
fn map_segment(row: &rusqlite::Row<'_>) -> rusqlite::Result<SegmentRecord> {
    Ok(SegmentRecord {
        track_id: row.get::<_, i64>(0)? as u32,
        index: row.get::<_, i64>(1)? as u32,
        path: row.get(2)?,
        started_at_unix_ms: row.get::<_, i64>(3)? as u64,
        frames: row.get::<_, i64>(4)? as u64,
        size_bytes: row.get::<_, i64>(5)? as u64,
        sha256: row.get(6)?,
        chain_link: row.get(7)?,
    })
}

/// Маппинг строки sessions в [`SessionRecord`] (внутри возвращает Result, чтобы
/// неизвестный код статуса не паниковал).
fn map_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<SessionRecord, StoreError>> {
    let status_code: String = row.get(3)?;
    let upload_code: String = row.get(11)?;
    let parsed = (|| {
        let status = SessionStatus::from_code(&status_code)
            .ok_or_else(|| StoreError::Db(format!("неизвестный статус сессии: {status_code}")))?;
        let upload_status = UploadStatus::from_code(&upload_code)
            .ok_or_else(|| StoreError::Db(format!("неизвестный upload_status: {upload_code}")))?;
        Ok(SessionRecord {
            id: row.get(0)?,
            dir: row.get(1)?,
            started_at_unix_ms: row.get::<_, i64>(2)? as u64,
            status,
            station_id: row.get(4)?,
            operator_id: row.get(5)?,
            adjudication_ref: row.get(6)?,
            sample_rate_hz: row.get::<_, i64>(7)? as u32,
            channels: row.get::<_, i64>(8)? as u16,
            bit_depth: row.get::<_, i64>(9)? as u16,
            final_chain_link: row.get(10)?,
            upload_status,
            server_integrity_verified: row.get::<_, i64>(12)? != 0,
            confirmed_at_unix_ms: row.get::<_, Option<i64>>(13)?.map(|v| v as u64),
            local_purged_at_unix_ms: row.get::<_, Option<i64>>(14)?.map(|v| v as u64),
            upload_paused: row.get::<_, i64>(15)? != 0,
        })
    })();
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session(id: &str) -> SessionRecord {
        SessionRecord::new(
            id,
            format!("/rec/{id}"),
            1_700_000_000_000,
            "station-1",
            "operator-7",
            44_100,
            1,
            16,
        )
    }

    #[test]
    fn insert_and_read_session() {
        let store = ManifestStore::in_memory().unwrap();
        let s = sample_session("sess-1");
        store.insert_session(&s).unwrap();
        let back = store.get_session("sess-1").unwrap().unwrap();
        assert_eq!(back, s);
        assert!(store.get_session("absent").unwrap().is_none());
    }

    #[test]
    fn append_segments_in_order() {
        let store = ManifestStore::in_memory().unwrap();
        store.insert_session(&sample_session("sess-1")).unwrap();
        for i in 1..=3u32 {
            store
                .append_segment(
                    "sess-1",
                    &SegmentRecord {
                        track_id: 0,
                        index: i,
                        path: format!("seg-{i}.wav.enc"),
                        started_at_unix_ms: 1_700_000_000_000 + i as u64,
                        frames: 1000 * i as u64,
                        size_bytes: 2000 * i as u64,
                        sha256: format!("hash{i}"),
                        chain_link: format!("link{i}"),
                    },
                )
                .unwrap();
        }
        let segs = store.get_segments("sess-1").unwrap();
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].index, 1);
        assert_eq!(segs[2].frames, 3000);
    }

    #[test]
    fn events_get_monotonic_seq() {
        let store = ManifestStore::in_memory().unwrap();
        store.insert_session(&sample_session("sess-1")).unwrap();
        let s1 = store
            .append_event("sess-1", &RecordingEvent::new(EventKind::SessionStarted, 1))
            .unwrap();
        let s2 = store
            .append_event(
                "sess-1",
                &RecordingEvent::with_detail(
                    EventKind::Paused,
                    2,
                    serde_json::json!({"reason": "device_lost"}),
                ),
            )
            .unwrap();
        assert_eq!((s1, s2), (1, 2));
        let events = store.get_events("sess-1").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.kind, EventKind::SessionStarted);
        assert_eq!(
            events[1].event.detail.as_ref().unwrap()["reason"],
            serde_json::json!("device_lost")
        );
    }

    #[test]
    fn setters_update_fields() {
        let store = ManifestStore::in_memory().unwrap();
        store.insert_session(&sample_session("sess-1")).unwrap();
        store.set_status("sess-1", SessionStatus::Stopped).unwrap();
        store.set_final_chain_link("sess-1", "final-link").unwrap();
        store
            .set_upload_status("sess-1", UploadStatus::Uploaded)
            .unwrap();
        store
            .set_adjudication_ref("sess-1", Some("adj-42"))
            .unwrap();
        store.set_upload_paused("sess-1", true).unwrap();
        let s = store.get_session("sess-1").unwrap().unwrap();
        assert_eq!(s.status, SessionStatus::Stopped);
        assert_eq!(s.final_chain_link.as_deref(), Some("final-link"));
        assert_eq!(s.upload_status, UploadStatus::Uploaded);
        assert_eq!(s.adjudication_ref.as_deref(), Some("adj-42"));
        assert!(s.upload_paused);
    }

    #[test]
    fn setter_on_missing_session_errors() {
        let store = ManifestStore::in_memory().unwrap();
        assert!(matches!(
            store.set_status("absent", SessionStatus::Stopped),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn delete_segments_and_events_keeps_session_tombstone() {
        let store = ManifestStore::in_memory().unwrap();
        store.insert_session(&sample_session("sess-1")).unwrap();
        store
            .append_segment(
                "sess-1",
                &SegmentRecord {
                    track_id: 0,
                    index: 1,
                    path: "seg-1.wav.enc".into(),
                    started_at_unix_ms: 1,
                    frames: 1,
                    size_bytes: 1,
                    sha256: "h".into(),
                    chain_link: "l".into(),
                },
            )
            .unwrap();
        store
            .append_event("sess-1", &RecordingEvent::new(EventKind::Stopped, 1))
            .unwrap();
        store.delete_segments_and_events("sess-1").unwrap();
        assert!(store.get_segments("sess-1").unwrap().is_empty());
        assert!(store.get_events("sess-1").unwrap().is_empty());
        // Сессия остаётся для истории.
        assert!(store.get_session("sess-1").unwrap().is_some());
    }
}
