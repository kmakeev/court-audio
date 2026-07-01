//! Оркестрация многоканального захвата (этап 09 — `promts/09_multichannel.md`,
//! шаги 2/4/7). Переиспользует одноканальную [`CaptureSession`] (этап 01) — по
//! одной на дорожку: каждая пишет свой канал в подкаталог `track-<id>-<role>/`
//! с собственным журналом и надёжностью. Обрыв устройства одной дорожки роняет
//! только её сессию (per-track supervisor), остальные продолжают писать —
//! deliverable 7 «надёжность на канал».
//!
//! Общий таймкод: все дорожки стартуют с одним `started_at_unix_ms` (шаг 2).
//! Карта дорожек персистится в `tracks.json` (корень сессии) — по ней
//! реконсиляция ([`crate::store::reconcile`]) наполняет манифест дорожками и
//! per-track сегментами.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::audio::capture::{CaptureParams, CaptureSession, LevelEvent, ReliabilityConfig};
use crate::audio::tracks::ResolvedTrack;
use crate::audio::AudioError;
use crate::recorder::segment_writer::SegmentInfo;

/// Имя файла карты дорожек в корне каталога сессии.
pub const TRACKS_FILE_NAME: &str = "tracks.json";

/// Персистируемая карта дорожек сессии (для реконсиляции и UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackMap {
    pub tracks: Vec<TrackMapEntry>,
}

/// Одна дорожка в карте: стабильный id, роль/метка, источник и подкаталог.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackMapEntry {
    pub track_id: u32,
    pub role: String,
    pub label: String,
    pub device: Option<String>,
    pub channel_index: u16,
    /// Подкаталог дорожки относительно каталога сессии.
    pub subdir: String,
}

/// Имя подкаталога дорожки: `track-<NN>-<role>` (стабильно, для реконсиляции).
pub fn track_subdir_name(track_id: u32, role: &str) -> String {
    format!("track-{track_id:02}-{}", sanitize(role))
}

/// Привести роль к безопасному имени каталога (латиница/цифры/-/_; прочее → `_`).
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Построить карту дорожек из разрешённого состава (`audio::tracks`).
pub fn track_map_from_resolved(tracks: &[ResolvedTrack]) -> TrackMap {
    TrackMap {
        tracks: tracks
            .iter()
            .map(|t| TrackMapEntry {
                track_id: t.track_id,
                role: t.role.clone(),
                label: t.label.clone(),
                device: t.device.clone(),
                channel_index: t.channel_index,
                subdir: track_subdir_name(t.track_id, &t.role),
            })
            .collect(),
    }
}

/// Записать карту дорожек в `tracks.json` каталога сессии.
pub fn write_track_map(session_dir: &Path, map: &TrackMap) -> Result<(), AudioError> {
    let json = serde_json::to_string_pretty(map)
        .map_err(|e| AudioError::Io(format!("сериализация tracks.json: {e}")))?;
    std::fs::create_dir_all(session_dir).map_err(|e| AudioError::Io(e.to_string()))?;
    std::fs::write(session_dir.join(TRACKS_FILE_NAME), json)
        .map_err(|e| AudioError::Io(e.to_string()))
}

/// Прочитать карту дорожек, если сессия многоканальная (`tracks.json` есть).
pub fn read_track_map(session_dir: &Path) -> Option<TrackMap> {
    let path = session_dir.join(TRACKS_FILE_NAME);
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Спецификация старта одной дорожки (готовится слоем IPC из настроек).
pub struct TrackStartSpec {
    pub track_id: u32,
    pub params: CaptureParams,
    pub reliability: ReliabilityConfig,
}

/// Дескриптор запущенной дорожки.
struct TrackHandle {
    track_id: u32,
    session: CaptureSession,
}

/// Оркестратор многоканального захвата: N независимых [`CaptureSession`].
pub struct MultiCapture {
    handles: Vec<TrackHandle>,
}

/// Тип эмиттера уровней (IPC подключает Tauri-эмиттер `audio_level`). События
/// уже несут `track_id` (проставляется consumer'ом из `ConsumerConfig`).
pub type LevelEmit = Arc<dyn Fn(LevelEvent) + Send + Sync + 'static>;

impl MultiCapture {
    /// Запустить все дорожки. При сбое любой — уже поднятые останавливаются
    /// (не оставляем полузапущенную сессию).
    pub fn start(specs: Vec<TrackStartSpec>, level_emit: LevelEmit) -> Result<Self, AudioError> {
        let mut handles: Vec<TrackHandle> = Vec::with_capacity(specs.len());
        for spec in specs {
            let emit = level_emit.clone();
            let level_cb = Box::new(move |lv: LevelEvent| emit(lv));
            match CaptureSession::start(spec.params, level_cb, spec.reliability) {
                Ok(session) => handles.push(TrackHandle {
                    track_id: spec.track_id,
                    session,
                }),
                Err(e) => {
                    for h in handles {
                        let _ = h.session.stop();
                    }
                    return Err(e);
                }
            }
        }
        Ok(Self { handles })
    }

    /// Поставить все дорожки на паузу (операторская пауза всей сессии).
    pub fn pause(&self) -> Result<(), AudioError> {
        for h in &self.handles {
            h.session.pause()?;
        }
        Ok(())
    }

    /// Возобновить все дорожки.
    pub fn resume(&self) -> Result<(), AudioError> {
        for h in &self.handles {
            h.session.resume()?;
        }
        Ok(())
    }

    /// На паузе ли сессия (по любой дорожке — они ставятся синхронно).
    pub fn is_paused(&self) -> bool {
        self.handles.first().is_some_and(|h| h.session.is_paused())
    }

    /// Число дорожек.
    pub fn track_count(&self) -> usize {
        self.handles.len()
    }

    /// Остановить все дорожки, вернуть их сегменты по `track_id`.
    pub fn stop(self) -> Result<Vec<(u32, Vec<SegmentInfo>)>, AudioError> {
        let mut out = Vec::with_capacity(self.handles.len());
        for h in self.handles {
            let segs = h.session.stop()?;
            out.push((h.track_id, segs));
        }
        Ok(out)
    }
}

/// Абсолютный подкаталог дорожки в каталоге сессии.
pub fn track_dir(session_dir: &Path, entry: &TrackMapEntry) -> PathBuf {
    session_dir.join(&entry.subdir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(track_id: u32, role: &str, ch: u16) -> ResolvedTrack {
        ResolvedTrack {
            track_id,
            device: Some(format!("dev-{track_id}")),
            channel_index: ch,
            role: role.to_string(),
            label: role.to_string(),
        }
    }

    #[test]
    fn subdir_name_is_stable_and_padded() {
        assert_eq!(track_subdir_name(0, "judge"), "track-00-judge");
        assert_eq!(track_subdir_name(12, "defense"), "track-12-defense");
    }

    #[test]
    fn subdir_name_sanitizes_role() {
        assert_eq!(track_subdir_name(1, "зал/room"), "track-01-____room");
    }

    #[test]
    fn track_map_roundtrips_through_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let map = track_map_from_resolved(&[resolved(0, "judge", 0), resolved(1, "defense", 1)]);
        write_track_map(tmp.path(), &map).unwrap();
        let back = read_track_map(tmp.path()).expect("карта читается");
        assert_eq!(back, map);
        assert_eq!(back.tracks[0].subdir, "track-00-judge");
        assert_eq!(back.tracks[1].channel_index, 1);
    }

    #[test]
    fn read_track_map_absent_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_track_map(tmp.path()).is_none());
    }
}
