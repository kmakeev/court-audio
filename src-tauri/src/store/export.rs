//! Экспорт манифеста записи в JSON (этап 03 — `promts/03_store_integrity.md`,
//! deliverable 5 / шаг 6; расширен многоканалом — этап 09).
//!
//! Сопровождает выгрузку (`06`): по этому документу сервер делает `verify`
//! (контракт `07`). Состав: метаданные сессии, **список дорожек** (роль +
//! per-track сегменты с SHA-256 и звеньями хеш-цепочки + финальное звено
//! дорожки), значимые события и параметры целостности. **Источник истины для
//! серверной верификации** — структуру менять синхронно с
//! `07_backend_integration.md` (и `promts/06_sync_agent.md`).
//!
//! Мультитрек-контракт (этап 09): одна `AudioRecording` на сессию, внутри —
//! `tracks[]`. Одноканальная (v1) запись — частный случай: одна дорожка
//! `track_id = 0` роль `single`.

use serde::{Deserialize, Serialize};

use super::manifest::{ManifestStore, SessionStatus, TrackRecord, UploadStatus};
use super::StoreError;
use crate::audio::tracks::SINGLE_TRACK_ROLE;
use crate::integrity::events::RecordingEvent;

/// Версия схемы манифеста записи. Инкремент — при изменении состава полей
/// (синхронно с серверным `verify`). v2 — многоканал: сегменты сгруппированы по
/// дорожкам (`tracks[]`).
pub const MANIFEST_VERSION: u32 = 2;

/// Полный экспортируемый манифест записи.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingManifest {
    pub manifest_version: u32,
    pub session: SessionMeta,
    pub integrity: IntegrityMeta,
    /// Дорожки записи (для одноканальной v1 — ровно одна, роль `single`).
    pub tracks: Vec<TrackEntry>,
    pub events: Vec<RecordingEvent>,
}

/// Метаданные сессии в манифесте.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub started_at_unix_ms: u64,
    pub status: SessionStatus,
    pub station_id: String,
    pub operator_id: String,
    /// Привязка к делу (`05`) — может отсутствовать.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adjudication_ref: Option<String>,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub bit_depth: u16,
    /// Число дорожек записи (многоканал — этап 09).
    pub track_count: u32,
    pub upload_status: UploadStatus,
    /// Финальное звено хеш-цепочки сессии (итог целостности; для одноканальной
    /// записи совпадает с финалом единственной дорожки).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_chain_link: Option<String>,
}

/// Дорожка в манифесте: роль + per-track целостность + свои сегменты.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackEntry {
    pub track_id: u32,
    /// Роль дорожки — вход диаризации W2.11 (`judge`/`defense`/…; `single` — v1).
    pub role: String,
    pub label: String,
    /// Финальное звено хеш-цепочки этой дорожки.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_chain_link: Option<String>,
    pub segments: Vec<SegmentEntry>,
}

/// Параметры целостности (для воспроизводимой серверной верификации).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntegrityMeta {
    /// Алгоритм хеша сегмента (`integrity.segment_hash`).
    pub segment_hash: String,
    /// Включена ли хеш-цепочка (`integrity.hash_chain`).
    pub hash_chain: bool,
}

/// Сегмент в манифесте: ровно то, что нужно серверу для `verify`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentEntry {
    /// Дорожка сегмента (дублирует группировку — для плоских потребителей).
    pub track_id: u32,
    pub index: u32,
    pub sha256: String,
    pub chain_link: String,
    pub size_bytes: u64,
    pub frames: u64,
    pub started_at_unix_ms: u64,
}

impl RecordingManifest {
    /// Сериализовать в компактный JSON.
    pub fn to_json(&self) -> Result<String, StoreError> {
        Ok(serde_json::to_string(self)?)
    }

    /// Сериализовать в человекочитаемый JSON (для файла-спутника записи).
    pub fn to_json_pretty(&self) -> Result<String, StoreError> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// Собрать манифест записи из SQLite-манифеста. `segment_hash`/`hash_chain` —
/// из `Settings.integrity.*` (без «магических чисел»).
///
/// Дорожки берутся из таблицы `tracks`; если их нет (сессия, наполненная до
/// многоканала, либо тесты без явных дорожек) — синтезируется одна дорожка
/// `track_id = 0` роль `single` со всеми сегментами сессии (обратная
/// совместимость v1).
pub fn build_manifest(
    store: &ManifestStore,
    session_id: &str,
    segment_hash: &str,
    hash_chain: bool,
) -> Result<RecordingManifest, StoreError> {
    let session = store
        .get_session(session_id)?
        .ok_or_else(|| StoreError::NotFound(format!("сессия {session_id}")))?;

    let track_records = store.get_tracks(session_id)?;
    let tracks = if track_records.is_empty() {
        // Легаси/тесты без явных дорожек: единственная дорожка track_id=0.
        let segments = segment_entries(store, session_id, 0)?;
        vec![TrackEntry {
            track_id: 0,
            role: SINGLE_TRACK_ROLE.to_string(),
            label: String::new(),
            final_chain_link: session.final_chain_link.clone(),
            segments,
        }]
    } else {
        let mut out = Vec::with_capacity(track_records.len());
        for t in track_records {
            let segments = segment_entries(store, session_id, t.track_id)?;
            out.push(track_entry(&t, segments));
        }
        out
    };

    let events = store
        .get_events(session_id)?
        .into_iter()
        .map(|e| e.event)
        .collect();

    Ok(RecordingManifest {
        manifest_version: MANIFEST_VERSION,
        session: SessionMeta {
            id: session.id,
            started_at_unix_ms: session.started_at_unix_ms,
            status: session.status,
            station_id: session.station_id,
            operator_id: session.operator_id,
            adjudication_ref: session.adjudication_ref,
            sample_rate_hz: session.sample_rate_hz,
            channels: session.channels,
            bit_depth: session.bit_depth,
            track_count: tracks.len() as u32,
            upload_status: session.upload_status,
            final_chain_link: session.final_chain_link,
        },
        integrity: IntegrityMeta {
            segment_hash: segment_hash.to_string(),
            hash_chain,
        },
        tracks,
        events,
    })
}

fn segment_entries(
    store: &ManifestStore,
    session_id: &str,
    track_id: u32,
) -> Result<Vec<SegmentEntry>, StoreError> {
    Ok(store
        .get_track_segments(session_id, track_id)?
        .into_iter()
        .map(|s| SegmentEntry {
            track_id: s.track_id,
            index: s.index,
            sha256: s.sha256,
            chain_link: s.chain_link,
            size_bytes: s.size_bytes,
            frames: s.frames,
            started_at_unix_ms: s.started_at_unix_ms,
        })
        .collect())
}

fn track_entry(t: &TrackRecord, segments: Vec<SegmentEntry>) -> TrackEntry {
    TrackEntry {
        track_id: t.track_id,
        role: t.role.clone(),
        label: t.label.clone(),
        final_chain_link: t.final_chain_link.clone(),
        segments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::events::EventKind;
    use crate::integrity::hash;
    use crate::store::manifest::{SegmentRecord, SessionRecord};

    fn seed_store() -> ManifestStore {
        let store = ManifestStore::in_memory().unwrap();
        store
            .insert_session(&SessionRecord::new(
                "sess-1",
                "/rec/sess-1",
                1_700_000_000_000,
                "station-1",
                "operator-7",
                44_100,
                1,
                16,
            ))
            .unwrap();

        // Два сегмента с реальными хешами и корректной цепочкой (дорожка 0).
        let hashes = vec![hash::sha256_bytes(b"seg1"), hash::sha256_bytes(b"seg2")];
        let chain = hash::build_chain(&hashes);
        for (i, (h, link)) in hashes.iter().zip(chain.iter()).enumerate() {
            store
                .append_segment(
                    "sess-1",
                    &SegmentRecord {
                        track_id: 0,
                        index: (i + 1) as u32,
                        path: format!("seg-{}.wav.enc", i + 1),
                        started_at_unix_ms: 1_700_000_000_000 + i as u64,
                        frames: 1_323_000,
                        size_bytes: 2_646_044,
                        sha256: h.clone(),
                        chain_link: link.clone(),
                    },
                )
                .unwrap();
        }
        store
            .set_final_chain_link("sess-1", chain.last().unwrap())
            .unwrap();
        store
            .append_event("sess-1", &RecordingEvent::new(EventKind::SessionStarted, 1))
            .unwrap();
        store
            .append_event("sess-1", &RecordingEvent::new(EventKind::Stopped, 2))
            .unwrap();
        store
    }

    /// Двухдорожечная сессия: судья (2 сегмента) + защита (1 сегмент), у каждой
    /// своя корректная хеш-цепочка.
    fn seed_multitrack() -> ManifestStore {
        let store = ManifestStore::in_memory().unwrap();
        store
            .insert_session(&SessionRecord::new(
                "mt", "/rec/mt", 1_700_000_000_000, "station-1", "operator-7", 48_000, 1, 16,
            ))
            .unwrap();
        for (tid, role, n) in [(0u32, "judge", 2usize), (1u32, "defense", 1usize)] {
            store
                .insert_track(
                    "mt",
                    &TrackRecord {
                        track_id: tid,
                        role: role.to_string(),
                        label: role.to_string(),
                        source_device: None,
                        source_channel: tid as u16,
                        final_chain_link: None,
                    },
                )
                .unwrap();
            let hashes: Vec<String> = (0..n)
                .map(|i| hash::sha256_bytes(format!("t{tid}-s{i}").as_bytes()))
                .collect();
            let chain = hash::build_chain(&hashes);
            for (i, (h, link)) in hashes.iter().zip(chain.iter()).enumerate() {
                store
                    .append_segment(
                        "mt",
                        &SegmentRecord {
                            track_id: tid,
                            index: (i + 1) as u32,
                            path: format!("t{tid}/seg-{}.wav.enc", i + 1),
                            started_at_unix_ms: 1_700_000_000_000 + i as u64,
                            frames: 480_000,
                            size_bytes: 960_044,
                            sha256: h.clone(),
                            chain_link: link.clone(),
                        },
                    )
                    .unwrap();
            }
            store
                .set_track_final_chain_link("mt", tid, chain.last().unwrap())
                .unwrap();
        }
        store
    }

    #[test]
    fn single_track_manifest_has_one_track_with_segments() {
        let store = seed_store();
        let m = build_manifest(&store, "sess-1", "sha256", true).unwrap();
        assert_eq!(m.manifest_version, MANIFEST_VERSION);
        assert_eq!(m.tracks.len(), 1);
        assert_eq!(m.session.track_count, 1);
        assert_eq!(m.tracks[0].role, SINGLE_TRACK_ROLE);
        assert_eq!(m.tracks[0].segments.len(), 2);
        assert_eq!(m.events.len(), 2);
        assert_eq!(
            m.session.final_chain_link,
            m.tracks[0].final_chain_link,
        );
    }

    #[test]
    fn manifest_is_sufficient_for_per_track_server_verify() {
        let store = seed_store();
        let m = build_manifest(&store, "sess-1", "sha256", true).unwrap();
        let track = &m.tracks[0];
        let hashes: Vec<String> = track.segments.iter().map(|s| s.sha256.clone()).collect();
        let links: Vec<String> = track.segments.iter().map(|s| s.chain_link.clone()).collect();
        assert!(hash::verify_chain(
            &hashes,
            &links,
            track.final_chain_link.as_deref()
        ));
    }

    #[test]
    fn multitrack_manifest_carries_roles_and_per_track_chains() {
        let store = seed_multitrack();
        let m = build_manifest(&store, "mt", "sha256", true).unwrap();
        assert_eq!(m.session.track_count, 2);
        let roles: Vec<&str> = m.tracks.iter().map(|t| t.role.as_str()).collect();
        assert_eq!(roles, vec!["judge", "defense"]);
        assert_eq!(m.tracks[0].segments.len(), 2);
        assert_eq!(m.tracks[1].segments.len(), 1);
        // Каждая дорожка верифицируется независимо.
        for t in &m.tracks {
            let hashes: Vec<String> = t.segments.iter().map(|s| s.sha256.clone()).collect();
            let links: Vec<String> = t.segments.iter().map(|s| s.chain_link.clone()).collect();
            assert!(hash::verify_chain(&hashes, &links, t.final_chain_link.as_deref()));
        }
    }

    #[test]
    fn tampering_one_track_does_not_break_others() {
        let store = seed_multitrack();
        let m = build_manifest(&store, "mt", "sha256", true).unwrap();
        // Портим первый сегмент дорожки judge — её цепочка ломается…
        let mut judge = m.tracks[0].clone();
        judge.segments[0].sha256 = hash::sha256_bytes(b"tampered");
        let jh: Vec<String> = judge.segments.iter().map(|s| s.sha256.clone()).collect();
        let jl: Vec<String> = judge.segments.iter().map(|s| s.chain_link.clone()).collect();
        assert!(!hash::verify_chain(&jh, &jl, judge.final_chain_link.as_deref()));
        // …а дорожка defense остаётся валидной.
        let d = &m.tracks[1];
        let dh: Vec<String> = d.segments.iter().map(|s| s.sha256.clone()).collect();
        let dl: Vec<String> = d.segments.iter().map(|s| s.chain_link.clone()).collect();
        assert!(hash::verify_chain(&dh, &dl, d.final_chain_link.as_deref()));
    }

    #[test]
    fn manifest_json_roundtrips() {
        let store = seed_multitrack();
        let m = build_manifest(&store, "mt", "sha256", true).unwrap();
        let json = m.to_json().unwrap();
        assert!(json.contains("\"tracks\""));
        assert!(json.contains("\"role\""));
        assert!(json.contains("\"chain_link\""));
        let back: RecordingManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn missing_session_errors() {
        let store = ManifestStore::in_memory().unwrap();
        assert!(matches!(
            build_manifest(&store, "absent", "sha256", true),
            Err(StoreError::NotFound(_))
        ));
    }
}
