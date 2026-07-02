//! Манифест экспортной копии (этап 10.2). Оборачивает существующий
//! [`crate::store::export::RecordingManifest`] (источник истины для
//! серверной верификации, этапы 03/09/10) экспортными полями: кто/когда
//! собрал копию, состав/формат, и **файловые** SHA-256 реально выданных
//! аудиофайлов. Хеши сегментов (`recording.tracks[*].segments[*].sha256`) —
//! это хеши ДО склейки/кодека; `files[*].sha256` — хеши уже склеенных/
//! перекодированных файлов пакета. Обе величины разные, обе нужны получателю
//! для независимой проверки целостности (критерий приёмки промта).

use serde::{Deserialize, Serialize};

use super::ExportError;
use crate::store::export::{self as recording_export, RecordingManifest};
use crate::store::manifest::ManifestStore;

/// Версия схемы манифеста копии (независима от `MANIFEST_VERSION`
/// вложенного `RecordingManifest`).
pub const COPY_MANIFEST_VERSION: u32 = 1;

/// Один файл экспортного пакета: относительный путь + SHA-256 + размер.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopyFileEntry {
    /// Относительный путь внутри пакета, напр. `audio/judge.wav`.
    pub name: String,
    pub sha256: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Полный манифест экспортной копии.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopyManifest {
    pub copy_manifest_version: u32,
    pub exported_at_unix_ms: u64,
    pub exported_by_operator_id: String,
    /// Состав пакета: `{"kind":"all_tracks"}` / `{"kind":"mix"}` /
    /// `{"kind":"track","track_id":N,"role":"..."}`.
    pub composition: serde_json::Value,
    /// Формат аудиофайлов пакета (`wav_pcm`/`flac`).
    pub format: String,
    pub files: Vec<CopyFileEntry>,
    /// Полный манифест записи (этапы 03/09/10) — источник истины для
    /// независимой верификации получателем без доступа к станции.
    pub recording: RecordingManifest,
}

impl CopyManifest {
    pub fn to_json_pretty(&self) -> Result<String, ExportError> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// Собрать манифест экспортной копии: обернуть
/// `store::export::build_manifest` экспортными полями.
#[allow(clippy::too_many_arguments)]
pub fn build_copy_manifest(
    store: &ManifestStore,
    session_id: &str,
    segment_hash: &str,
    hash_chain: bool,
    exported_by_operator_id: &str,
    exported_at_unix_ms: u64,
    composition: serde_json::Value,
    format: &str,
    files: Vec<CopyFileEntry>,
) -> Result<CopyManifest, ExportError> {
    let recording = recording_export::build_manifest(store, session_id, segment_hash, hash_chain)?;
    Ok(CopyManifest {
        copy_manifest_version: COPY_MANIFEST_VERSION,
        exported_at_unix_ms,
        exported_by_operator_id: exported_by_operator_id.to_string(),
        composition,
        format: format.to_string(),
        files,
        recording,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::events::{EventKind, RecordingEvent};
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
                        frames: 44_100,
                        size_bytes: 88_244,
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

    fn sample_files() -> Vec<CopyFileEntry> {
        vec![CopyFileEntry {
            name: "audio/single.wav".to_string(),
            sha256: hash::sha256_bytes(b"joined-audio-bytes"),
            size_bytes: 12_345,
            track_id: Some(0),
            role: Some("single".to_string()),
        }]
    }

    #[test]
    fn build_copy_manifest_embeds_recording_manifest() {
        let store = seed_store();
        let direct =
            recording_export::build_manifest(&store, "sess-1", "sha256", true).unwrap();
        let copy = build_copy_manifest(
            &store,
            "sess-1",
            "sha256",
            true,
            "op-7",
            1_700_000_100_000,
            serde_json::json!({"kind": "all_tracks"}),
            "wav_pcm",
            sample_files(),
        )
        .unwrap();
        assert_eq!(copy.recording, direct);
    }

    #[test]
    fn build_copy_manifest_file_hashes_match_input() {
        let store = seed_store();
        let files = sample_files();
        let copy = build_copy_manifest(
            &store,
            "sess-1",
            "sha256",
            true,
            "op-7",
            1_700_000_100_000,
            serde_json::json!({"kind": "mix"}),
            "flac",
            files.clone(),
        )
        .unwrap();
        assert_eq!(copy.files, files);
        assert_eq!(copy.format, "flac");
    }

    #[test]
    fn copy_manifest_markers_match_session_manifest() {
        let store = seed_store();
        let copy = build_copy_manifest(
            &store,
            "sess-1",
            "sha256",
            true,
            "op-7",
            1_700_000_100_000,
            serde_json::json!({"kind": "all_tracks"}),
            "wav_pcm",
            sample_files(),
        )
        .unwrap();
        let direct =
            recording_export::build_manifest(&store, "sess-1", "sha256", true).unwrap();
        assert_eq!(copy.recording.annotations.markers, direct.annotations.markers);
        assert_eq!(
            copy.recording.annotations.role_spans,
            direct.annotations.role_spans
        );
    }

    #[test]
    fn copy_manifest_json_roundtrips() {
        let store = seed_store();
        let copy = build_copy_manifest(
            &store,
            "sess-1",
            "sha256",
            true,
            "op-7",
            1_700_000_100_000,
            serde_json::json!({"kind": "track", "track_id": 0, "role": "single"}),
            "wav_pcm",
            sample_files(),
        )
        .unwrap();
        let json = copy.to_json_pretty().unwrap();
        assert!(json.contains("\"copy_manifest_version\""));
        assert!(json.contains("\"recording\""));
        let back: CopyManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, copy);
    }

    #[test]
    fn build_copy_manifest_errors_on_missing_session() {
        let store = ManifestStore::in_memory().unwrap();
        let err = build_copy_manifest(
            &store,
            "absent",
            "sha256",
            true,
            "op-7",
            1,
            serde_json::json!({"kind": "all_tracks"}),
            "wav_pcm",
            vec![],
        )
        .unwrap_err();
        assert!(matches!(err, ExportError::Store(_)));
    }
}
