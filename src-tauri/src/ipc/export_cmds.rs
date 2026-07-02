//! Tauri-команды мастера экспорта (этап 10.2 — `promts/10_2_export.md`).
//!
//! Тонкая обвязка над `crate::export` (сборка пакета/FLAC/HTML-плеер/DVD —
//! Tauri-agnostic, тестируется отдельно) + `store::manifest` — тот же
//! паттерн, что `ipc::player_cmds`: реконсиляция каталога → таймлайны
//! дорожек → резолв ключа шифрования → работа → аудит. Экспорт — управляемое
//! действие: `Settings.export.policy` проверяется **здесь** (не только в
//! UI), и отказ, и успех одинаково журналируются (`EventKind::ExportCreated`).
//!
//! Команды: `export_session_info`, `export_build_package`,
//! `export_dvd_drive_status`, `export_burn_dvd`. Событие `export_progress` —
//! прогресс сборки пакета для UI.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::export::dvd::{self, DriveInfo, DvdBurner};
use crate::export::manifest::CopyManifest;
use crate::export::package::{self, BuildRequest, PackageFormat, PackageTrackPlan, PlannedTrack};
use crate::export::{audit, check_policy, PermissionOutcome};
use crate::ipc::audio_cmds::operator_identity;
use crate::ipc::{load_settings, resolve_storage_root, MANIFEST_FILE};
use crate::player::timeline::Timeline;
use crate::reliability::watchdog::now_unix_ms;
use crate::store::case_binding::AdjudicationRef;
use crate::store::crypto;
use crate::store::manifest::{ManifestStore, SessionRecord, TrackRecord};
use crate::store::reconcile;

/// Состав пакета, выбранный оператором в мастере.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportComposition {
    AllTracks,
    Mix,
    Track { track_id: u32 },
}

/// Формат аудиофайлов пакета.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    WavPcm,
    Flac,
}

impl From<ExportFormat> for PackageFormat {
    fn from(f: ExportFormat) -> Self {
        match f {
            ExportFormat::WavPcm => PackageFormat::WavPcm,
            ExportFormat::Flac => PackageFormat::Flac,
        }
    }
}

/// Дорожка сессии для шага «Состав» мастера.
#[derive(Debug, Clone, Serialize)]
pub struct ExportTrackView {
    pub track_id: u32,
    pub role: String,
    pub label: String,
}

/// Ответ `export_session_info`.
#[derive(Debug, Clone, Serialize)]
pub struct ExportSessionInfo {
    pub session_id: String,
    pub adjudication_ref: Option<String>,
    pub started_at_unix_ms: u64,
    pub duration_ms: u64,
    pub tracks: Vec<ExportTrackView>,
    pub integrity_ok: bool,
}

/// Один файл в ответе сборки пакета.
#[derive(Debug, Clone, Serialize)]
pub struct ExportFileView {
    pub name: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Ответ `export_build_package`.
#[derive(Debug, Clone, Serialize)]
pub struct ExportResult {
    pub package_dir: String,
    pub files: Vec<ExportFileView>,
    pub manifest_path: String,
    pub player_path: String,
}

/// Найденный привод DVD (шаг «Назначение»).
#[derive(Debug, Clone, Serialize)]
pub struct DvdDriveView {
    pub id: String,
    pub label: String,
}

/// Итог прожига DVD.
#[derive(Debug, Clone, Serialize)]
pub struct DvdBurnResultView {
    pub verified: bool,
    pub drive: String,
}

const EVENT_EXPORT_PROGRESS: &str = "export_progress";

#[derive(Debug, Clone, Serialize)]
struct ExportProgressEvent {
    stage: String,
    percent: u8,
}

/// Открыть сессию `dir` для мастера экспорта: реконсилировать манифест,
/// вернуть дорожки/длительность/целостность. Без побочных эффектов — сам
/// просмотр состава дорожек ещё не выдаёт ПДн наружу; аудируется только
/// реальная сборка пакета (`export_build_package`).
#[tauri::command]
pub fn export_session_info(app: AppHandle, dir: String) -> Result<ExportSessionInfo, String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;
    let store = ManifestStore::open(&root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;
    let session_id = reconcile::reconcile_session(&store, &PathBuf::from(&dir))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("каталог не содержит начатой сессии: {dir}"))?;

    let session = store
        .get_session(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("сессия {session_id} не найдена"))?;

    let track_records = store.resolve_tracks(&session_id).map_err(|e| e.to_string())?;
    let all_segments = store.get_segments(&session_id).map_err(|e| e.to_string())?;

    let first_timeline = Timeline::build(&all_segments, track_records[0].track_id, session.sample_rate_hz);
    let duration_ms = frames_to_ms(first_timeline.total_frames, session.sample_rate_hz);

    let segments_hashed = all_segments.iter().filter(|s| !s.sha256.is_empty()).count();
    let integrity_ok = !all_segments.is_empty() && segments_hashed == all_segments.len();

    Ok(ExportSessionInfo {
        session_id,
        adjudication_ref: session.adjudication_ref,
        started_at_unix_ms: session.started_at_unix_ms,
        duration_ms,
        tracks: track_records
            .iter()
            .map(|t| ExportTrackView {
                track_id: t.track_id,
                role: t.role.clone(),
                label: t.label.clone(),
            })
            .collect(),
        integrity_ok,
    })
}

/// Собрать экспортный пакет. Проверяет `Settings.export.policy`:
/// `forbidden` → `Err` + аудит `result:"denied"`; `requires_confirmation`
/// без `confirmed:true` → `Err` без аудита (обычный шаг мастера, не попытка
/// обхода); иначе строит пакет, эмитит `export_progress`, журналирует успех.
/// `destination_dir: None` — временный каталог станции для последующего
/// прожига DVD (`export_burn_dvd`).
#[tauri::command]
pub fn export_build_package(
    app: AppHandle,
    dir: String,
    composition: ExportComposition,
    format: ExportFormat,
    destination_dir: Option<String>,
    confirmed: bool,
) -> Result<ExportResult, String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;
    let store = ManifestStore::open(&root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;
    let session_id = reconcile::reconcile_session(&store, &PathBuf::from(&dir))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("каталог не содержит начатой сессии: {dir}"))?;

    let operator_id = operator_identity(&app);
    let at = now_unix_ms();

    match check_policy(settings.export.policy, confirmed) {
        PermissionOutcome::Denied => {
            let _ = audit::record_export(
                &store,
                &session_id,
                at,
                serde_json::json!({
                    "operator_id": operator_id,
                    "result": "denied",
                    "reason": "policy_forbidden",
                }),
            );
            return Err("экспорт запрещён администратором станции".to_string());
        }
        PermissionOutcome::NeedsConfirmation => {
            return Err("экспорт требует подтверждения оператора".to_string());
        }
        PermissionOutcome::Allowed => {}
    }

    let session = store
        .get_session(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("сессия {session_id} не найдена"))?;

    let track_records = store.resolve_tracks(&session_id).map_err(|e| e.to_string())?;
    let all_segments = store.get_segments(&session_id).map_err(|e| e.to_string())?;

    let key = if settings.storage.encrypt_at_rest {
        crypto::resolve_station_key(settings.storage.key_source, &root).ok()
    } else {
        None
    };

    let sample_rate_hz = session.sample_rate_hz;
    let plan_track = |t: &TrackRecord| PlannedTrack {
        track_id: t.track_id,
        role: t.role.clone(),
        label: t.label.clone(),
        timeline: Timeline::build(&all_segments, t.track_id, sample_rate_hz),
    };

    let (plan, composition_detail) = match &composition {
        ExportComposition::AllTracks => (
            PackageTrackPlan::Separate(track_records.iter().map(plan_track).collect()),
            serde_json::json!({"kind": "all_tracks"}),
        ),
        ExportComposition::Mix => (
            PackageTrackPlan::Mix(track_records.iter().map(plan_track).collect()),
            serde_json::json!({"kind": "mix"}),
        ),
        ExportComposition::Track { track_id } => {
            let t = track_records
                .iter()
                .find(|t| t.track_id == *track_id)
                .ok_or_else(|| format!("дорожка {track_id} не найдена"))?;
            (
                PackageTrackPlan::Separate(vec![plan_track(t)]),
                serde_json::json!({"kind": "track", "track_id": track_id, "role": t.role}),
            )
        }
    };

    let package_dir = resolve_package_dir(&destination_dir, &root, &session);
    std::fs::create_dir_all(&package_dir).map_err(|e| e.to_string())?;

    let app_for_progress = app.clone();
    let build_req = BuildRequest {
        store: &store,
        session_id: &session_id,
        package_dir: &package_dir,
        key,
        plan,
        format: format.into(),
        segment_hash: &settings.integrity.segment_hash,
        hash_chain: settings.integrity.hash_chain,
        operator_id: &operator_id,
        exported_at_unix_ms: at,
        composition_detail: composition_detail.clone(),
        session_started_at_unix_ms: session.started_at_unix_ms,
        adjudication_ref: session.adjudication_ref.clone(),
        seek_step_seconds: settings.player.seek_step_seconds,
        playback_rates: settings.player.playback_rates.clone(),
    };

    let outcome = package::build_package(build_req, |stage, pct| {
        let _ = app_for_progress.emit(
            EVENT_EXPORT_PROGRESS,
            ExportProgressEvent {
                stage: stage.to_string(),
                percent: pct,
            },
        );
    })
    .map_err(|e| e.to_string())?;

    audit::record_export(
        &store,
        &session_id,
        at,
        serde_json::json!({
            "operator_id": operator_id,
            "result": "ok",
            "composition": composition_detail,
            "format": format,
            "destination": {"kind": "folder", "path": package_dir.to_string_lossy()},
            "files": outcome.files.len(),
        }),
    )
    .map_err(|e| e.to_string())?;

    Ok(ExportResult {
        package_dir: package_dir.to_string_lossy().into_owned(),
        files: outcome
            .files
            .iter()
            .map(|f| ExportFileView {
                name: f.name.clone(),
                sha256: f.sha256.clone(),
                size_bytes: f.size_bytes,
            })
            .collect(),
        manifest_path: outcome.manifest_path.to_string_lossy().into_owned(),
        player_path: outcome.player_path.to_string_lossy().into_owned(),
    })
}

/// Найти привод/утилиту прожига (шаг «Назначение»): `None` — привода/утилиты
/// нет или ОС не поддержана, экспорт в папку остаётся доступен (критерий
/// приёмки промта).
#[tauri::command]
pub fn export_dvd_drive_status() -> Result<Option<DvdDriveView>, String> {
    let burner = dvd::platform_burner();
    match burner.detect_drive() {
        Ok(Some(d)) => Ok(Some(DvdDriveView {
            id: d.id,
            label: d.label,
        })),
        Ok(None) => Ok(None),
        Err(dvd::DvdError::UnsupportedOs) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Прожечь уже собранный пакет (`package_dir` из `export_build_package`) на
/// DVD и верифицировать чтением+сверкой хешей. Отдельное журнальное событие
/// (`destination.kind = "dvd"`), независимое от первичной сборки в папку.
#[tauri::command]
pub fn export_burn_dvd(
    app: AppHandle,
    dir: String,
    package_dir: String,
    drive_id: String,
) -> Result<DvdBurnResultView, String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;
    let store = ManifestStore::open(&root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;
    let session_id = reconcile::reconcile_session(&store, &PathBuf::from(&dir))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("каталог не содержит начатой сессии: {dir}"))?;

    let package_path = PathBuf::from(&package_dir);
    let manifest_json = std::fs::read_to_string(package_path.join("manifest.json"))
        .map_err(|e| format!("не удалось прочитать манифест пакета: {e}"))?;
    let manifest: CopyManifest = serde_json::from_str(&manifest_json).map_err(|e| e.to_string())?;

    let burner = dvd::platform_burner();
    let drive = DriveInfo {
        id: drive_id.clone(),
        label: drive_id.clone(),
    };
    let label = package_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| session_id.clone());

    let operator_id = operator_identity(&app);
    let at = now_unix_ms();

    burner.burn(&package_path, &drive, &label).map_err(|e| e.to_string())?;
    let verified = dvd::verify_package_on_path(&package_path, &manifest.files).is_ok();

    audit::record_export(
        &store,
        &session_id,
        at,
        serde_json::json!({
            "operator_id": operator_id,
            "result": if verified { "ok" } else { "failed" },
            "destination": {"kind": "dvd", "drive": drive_id, "verified": verified},
        }),
    )
    .map_err(|e| e.to_string())?;

    Ok(DvdBurnResultView {
        verified,
        drive: drive_id,
    })
}

/// Каталог назначения пакета: явно выбранный оператором путь (папка/USB) —
/// подкаталог `<дело>_<дата>` внутри него; либо (для DVD, `None`) —
/// временный каталог станции, который прожигается и затем может быть удалён
/// вызывающим.
fn resolve_package_dir(
    destination_dir: &Option<String>,
    root: &Path,
    session: &SessionRecord,
) -> PathBuf {
    let folder_name = package_folder_name(session);
    match destination_dir {
        Some(d) => PathBuf::from(d).join(folder_name),
        None => root.join("export_staging").join(folder_name),
    }
}

fn package_folder_name(session: &SessionRecord) -> String {
    let case_part = session
        .adjudication_ref
        .as_deref()
        .and_then(|raw| AdjudicationRef::from_json(raw).ok())
        .and_then(|r| r.raw_number)
        .unwrap_or_else(|| session.id.clone());
    format!(
        "{}_{}",
        package::sanitize_filename(&case_part, 0),
        unix_ms_to_date_string(session.started_at_unix_ms)
    )
}

/// `YYYY-MM-DD` из Unix-миллисекунд (UTC), алгоритм Hinnant
/// `civil_from_days` — не тянуть внешний time-крейт ради одной даты в имени
/// каталога пакета.
fn unix_ms_to_date_string(unix_ms: u64) -> String {
    let days = (unix_ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn frames_to_ms(frames: u64, sample_rate_hz: u32) -> u64 {
    if sample_rate_hz == 0 {
        return 0;
    }
    frames * 1000 / sample_rate_hz as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_ms_to_date_known_dates() {
        assert_eq!(unix_ms_to_date_string(1_700_000_000_000), "2023-11-14");
        assert_eq!(unix_ms_to_date_string(0), "1970-01-01");
    }

    #[test]
    fn package_folder_name_uses_raw_number_when_bound() {
        let mut s = SessionRecord::new(
            "sess-1",
            "/rec/sess-1",
            1_700_000_000_000,
            "station-1",
            "op-1",
            44_100,
            1,
            16,
        );
        let ar = AdjudicationRef::manual("№ 1-123/2026", None).unwrap();
        s.adjudication_ref = Some(ar.to_json().unwrap());
        let name = package_folder_name(&s);
        assert_eq!(name, "1-123_2026_2023-11-14");
    }

    #[test]
    fn package_folder_name_falls_back_to_session_id_when_unbound() {
        let s = SessionRecord::new(
            "sess-1",
            "/rec/sess-1",
            1_700_000_000_000,
            "station-1",
            "op-1",
            44_100,
            1,
            16,
        );
        let name = package_folder_name(&s);
        assert_eq!(name, "sess-1_2023-11-14");
    }
}
