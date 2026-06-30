//! Экспорт манифеста записи в JSON (этап 03 — `promts/03_store_integrity.md`,
//! deliverable 5 / шаг 6).
//!
//! Сопровождает выгрузку (`06`): по этому документу сервер делает `verify`
//! (контракт `07`). Состав: метаданные сессии, список сегментов с SHA-256 и
//! звеньями хеш-цепочки, финальное звено, значимые события и параметры
//! целостности. **Источник истины для серверной верификации** — структуру
//! менять синхронно с `07_backend_integration.md`.

use serde::{Deserialize, Serialize};

use super::manifest::{ManifestStore, SessionStatus, UploadStatus};
use super::StoreError;
use crate::integrity::events::RecordingEvent;

/// Версия схемы манифеста записи. Инкремент — при изменении состава полей
/// (синхронно с серверным `verify`).
pub const MANIFEST_VERSION: u32 = 1;

/// Полный экспортируемый манифест записи.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingManifest {
    pub manifest_version: u32,
    pub session: SessionMeta,
    pub integrity: IntegrityMeta,
    pub segments: Vec<SegmentEntry>,
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
    pub upload_status: UploadStatus,
    /// Финальное звено хеш-цепочки сессии (итог целостности).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_chain_link: Option<String>,
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
pub fn build_manifest(
    store: &ManifestStore,
    session_id: &str,
    segment_hash: &str,
    hash_chain: bool,
) -> Result<RecordingManifest, StoreError> {
    let session = store
        .get_session(session_id)?
        .ok_or_else(|| StoreError::NotFound(format!("сессия {session_id}")))?;
    let segments = store
        .get_segments(session_id)?
        .into_iter()
        .map(|s| SegmentEntry {
            index: s.index,
            sha256: s.sha256,
            chain_link: s.chain_link,
            size_bytes: s.size_bytes,
            frames: s.frames,
            started_at_unix_ms: s.started_at_unix_ms,
        })
        .collect();
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
            upload_status: session.upload_status,
            final_chain_link: session.final_chain_link,
        },
        integrity: IntegrityMeta {
            segment_hash: segment_hash.to_string(),
            hash_chain,
        },
        segments,
        events,
    })
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

        // Два сегмента с реальными хешами и корректной цепочкой.
        let hashes = vec![hash::sha256_bytes(b"seg1"), hash::sha256_bytes(b"seg2")];
        let chain = hash::build_chain(&hashes);
        for (i, (h, link)) in hashes.iter().zip(chain.iter()).enumerate() {
            store
                .append_segment(
                    "sess-1",
                    &SegmentRecord {
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

    #[test]
    fn manifest_has_segments_hashes_chain_and_events() {
        let store = seed_store();
        let m = build_manifest(&store, "sess-1", "sha256", true).unwrap();
        assert_eq!(m.manifest_version, MANIFEST_VERSION);
        assert_eq!(m.segments.len(), 2);
        assert_eq!(m.events.len(), 2);
        assert_eq!(m.integrity.segment_hash, "sha256");
        assert!(m.integrity.hash_chain);
        assert_eq!(
            m.session.final_chain_link.as_deref(),
            Some(m.segments.last().unwrap().chain_link.as_str())
        );
    }

    #[test]
    fn manifest_is_sufficient_for_server_verify() {
        // Сервер берёт sha256+chain_link из манифеста и проверяет цепочку.
        let store = seed_store();
        let m = build_manifest(&store, "sess-1", "sha256", true).unwrap();
        let hashes: Vec<String> = m.segments.iter().map(|s| s.sha256.clone()).collect();
        let links: Vec<String> = m.segments.iter().map(|s| s.chain_link.clone()).collect();
        assert!(hash::verify_chain(
            &hashes,
            &links,
            m.session.final_chain_link.as_deref()
        ));
    }

    #[test]
    fn manifest_json_roundtrips() {
        let store = seed_store();
        let m = build_manifest(&store, "sess-1", "sha256", true).unwrap();
        let json = m.to_json().unwrap();
        assert!(json.contains("\"manifest_version\""));
        assert!(json.contains("\"segments\""));
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
