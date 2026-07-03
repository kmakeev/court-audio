//! Read-only Tauri-команды-запросы для UI (этап 04 — `promts/04_ui_capture.md`).
//!
//! Прокидывают в интерфейс данные стора этапа 03 (манифест сессий) и
//! диагностику надёжности этапа 02 (свободное место, устройства, журнал
//! событий) — **без новой бизнес-логики**: только запросы к уже существующим
//! функциям ядра (`store::manifest`, `store::reconcile`,
//! `reliability::disk_monitor`, `audio::devices`). Слой IPC отдаёт UI один тип
//! ошибки (`String`) — ядро остаётся Tauri-agnostic.
//!
//! Команды: `list_sessions`, `diagnostics`. Запись/целостность/восстановление —
//! это ядро (01–03); здесь их не дублируем, лишь отображаем зафиксированное
//! состояние.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::AppHandle;

use crate::audio::devices::{list_input_devices, DeviceInfo};
use crate::integrity::annotations::{self, MarkerState, RoleSpanState};
use crate::ipc::{load_settings, resolve_storage_root, MANIFEST_FILE};
use crate::reliability::disk_monitor::{classify, free_space_mb, DiskStatus, DiskThresholds};
use crate::store::manifest::{EventRecord, ManifestStore, SessionRecord};
use crate::store::{reconcile, session_comment};

/// Открыть манифест станции в корне хранилища и реконсилировать все каталоги
/// сессий из их журналов (идемпотентно). Так в манифесте появляются сессии,
/// записанные ядром, без правок самого пути захвата.
fn open_and_reconcile(app: &AppHandle) -> Result<ManifestStore, String> {
    let settings = load_settings(app)?;
    let root = resolve_storage_root(app, &settings)?;
    let store = ManifestStore::open(&root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;
    reconcile_all(&store, &root);
    Ok(store)
}

/// Реконсилировать все подкаталоги корня хранилища. Ошибки отдельных сессий не
/// валят запрос целиком — пропускаем сбойный каталог (UI покажет остальное).
fn reconcile_all(store: &ManifestStore, root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return; // корень ещё не создан — сессий нет
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = reconcile::reconcile_session(store, &path);
        }
    }
}

/// Запись сессии для UI + производные (число сегментов и длительность). Сводка
/// поверх манифеста — не новая логика, лишь агрегация уже посчитанного.
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    #[serde(flatten)]
    pub record: SessionRecord,
    pub segment_count: u32,
    /// Длительность записи в секундах (сумма кадров сегментов / частота).
    pub duration_seconds: u64,
    /// Всего частей выгрузки (= сегментов, заявленных в `upload/init`); 0 — пока
    /// выгрузка не начиналась. Этап 06: прогресс «выгружается N%».
    pub upload_total_parts: u32,
    /// Сколько частей принято сервером.
    pub upload_sent_parts: u32,
}

/// Перечислить локальные сессии из манифеста (новые сверху). Источник данных
/// экрана «Сессии».
#[tauri::command]
pub fn list_sessions(app: AppHandle) -> Result<Vec<SessionView>, String> {
    let store = open_and_reconcile(&app)?;
    let records = store.list_sessions().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(records.len());
    for record in records {
        let segs = store.get_segments(&record.id).map_err(|e| e.to_string())?;
        let total_frames: u64 = segs.iter().map(|s| s.frames).sum();
        let duration_seconds = if record.sample_rate_hz > 0 {
            total_frames / record.sample_rate_hz as u64
        } else {
            0
        };
        // Прогресс выгрузки (этап 06): из трекинга частей `upload_parts`.
        let progress =
            crate::sync::queue::progress(&store, &record.id).map_err(|e| e.to_string())?;
        out.push(SessionView {
            segment_count: segs.len() as u32,
            duration_seconds,
            upload_total_parts: progress.total,
            upload_sent_parts: progress.sent,
            record,
        });
    }
    Ok(out)
}

/// Свободное место + классификация по порогам (для экрана «Диагностика»).
#[derive(Debug, Clone, Serialize)]
pub struct DiskInfo {
    pub free_mb: u64,
    /// `ok` / `low` / `critical`.
    pub status: &'static str,
    pub low_threshold_mb: u64,
    pub critical_mb: u64,
}

/// Сводка целостности последней сессии — зафиксированное в манифесте состояние
/// (не ре-верификация: это работа ядра, этап 03).
#[derive(Debug, Clone, Serialize)]
pub struct IntegritySummary {
    pub session_id: String,
    pub segments: u32,
    /// Сколько сегментов имеют посчитанный SHA-256.
    pub segments_hashed: u32,
    pub final_chain_link: Option<String>,
    /// Флаги политики целостности из настроек (реестр).
    pub hash_chain_enabled: bool,
    pub event_log_enabled: bool,
}

/// Идентичность станции и сборки (для экрана «Диагностика»).
#[derive(Debug, Clone, Serialize)]
pub struct StationInfo {
    pub app_version: String,
    pub storage_root: String,
    /// Идентификатор станции из последней сессии (наполнится с аутентификацией,
    /// этап `auth`); пусто — «не настроена».
    pub station_id: Option<String>,
}

/// Полезная нагрузка команды `diagnostics`.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsInfo {
    pub devices: Vec<DeviceInfo>,
    pub disk: DiskInfo,
    pub station: StationInfo,
    /// Последняя сессия (если есть) — для блока событий/целостности.
    pub last_session: Option<SessionRecord>,
    /// Значимые события последней сессии (старт/пауза/обрыв/восстановление/стоп).
    pub recent_events: Vec<EventRecord>,
    pub integrity: Option<IntegritySummary>,
}

/// Собрать диагностику: устройства, свободное место, идентичность станции и
/// сводку по последней сессии (события + целостность). Всё — чтение уже
/// существующего состояния ядра.
#[tauri::command]
pub fn diagnostics(app: AppHandle) -> Result<DiagnosticsInfo, String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;

    let devices = list_input_devices().map_err(|e| e.to_string())?;

    // Свободное место по тому корня хранилища; если каталога ещё нет — 0/неизвестно.
    let thresholds = DiskThresholds {
        low_mb: settings.reliability.disk_low_threshold_mb,
        critical_mb: settings.reliability.disk_critical_mb,
    };
    let free_mb = free_space_mb(&root).unwrap_or(0);
    let disk = DiskInfo {
        free_mb,
        status: disk_status_code(classify(free_mb, thresholds)),
        low_threshold_mb: thresholds.low_mb,
        critical_mb: thresholds.critical_mb,
    };

    let store = ManifestStore::open(&root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;
    reconcile_all(&store, &root);
    let last_session = store
        .list_sessions()
        .map_err(|e| e.to_string())?
        .into_iter()
        .next();

    let (recent_events, integrity) = match &last_session {
        Some(s) => {
            let events = store.get_events(&s.id).map_err(|e| e.to_string())?;
            let segs = store.get_segments(&s.id).map_err(|e| e.to_string())?;
            let segments_hashed = segs.iter().filter(|seg| !seg.sha256.is_empty()).count() as u32;
            let summary = IntegritySummary {
                session_id: s.id.clone(),
                segments: segs.len() as u32,
                segments_hashed,
                final_chain_link: s.final_chain_link.clone(),
                hash_chain_enabled: settings.integrity.hash_chain,
                event_log_enabled: settings.integrity.event_log,
            };
            (events, Some(summary))
        }
        None => (Vec::new(), None),
    };

    let station = StationInfo {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        storage_root: root.to_string_lossy().into_owned(),
        station_id: last_session
            .as_ref()
            .map(|s| s.station_id.clone())
            .filter(|id| !id.is_empty()),
    };

    Ok(DiagnosticsInfo {
        devices,
        disk,
        station,
        last_session,
        recent_events,
        integrity,
    })
}

/// Стабильный строковый код статуса диска (как ожидает UI).
fn disk_status_code(status: DiskStatus) -> &'static str {
    match status {
        DiskStatus::Ok => "ok",
        DiskStatus::Low => "low",
        DiskStatus::Critical => "critical",
    }
}

/// Детальная карточка сессии (этап 10.6, deliverable 3). Read-only агрегат
/// зафиксированного состояния конкретной сессии для экрана «Карточка сессии»:
/// журнал событий, статус целостности, свёрнутая разметка (метки/роли),
/// комментарий оператора. **Без аудит-побочек** (в отличие от
/// `player_open_session`, который пишет `playback_accessed`) — это просмотр
/// метаданных, а не доступ к аудиосодержимому.
#[derive(Debug, Clone, Serialize)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub record: SessionRecord,
    pub segment_count: u32,
    pub duration_seconds: u64,
    pub integrity: IntegritySummary,
    pub events: Vec<EventRecord>,
    pub markers: Vec<MarkerState>,
    pub role_spans: Vec<RoleSpanState>,
    /// Свободный комментарий оператора (`store::session_comment`), если задан.
    pub comment: Option<String>,
}

/// Открыть манифест и реконсилировать конкретный каталог сессии (без обхода всего
/// корня — карточке нужна одна сессия). Возвращает store + резолвнутый `id`.
fn open_and_reconcile_one(
    app: &AppHandle,
    dir: &str,
) -> Result<(ManifestStore, String), String> {
    let settings = load_settings(app)?;
    let root = resolve_storage_root(app, &settings)?;
    let store = ManifestStore::open(&root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;
    let session_id = reconcile::reconcile_session(&store, &PathBuf::from(dir))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("каталог не содержит начатой сессии: {dir}"))?;
    Ok((store, session_id))
}

/// Детальная карточка сессии по её каталогу. Источник данных экрана «Карточка
/// сессии»; действия «Прослушать»/«Экспортировать» — отдельные команды (10.1/10.2).
#[tauri::command]
pub fn session_detail(app: AppHandle, dir: String) -> Result<SessionDetail, String> {
    let settings = load_settings(&app)?;
    let (store, session_id) = open_and_reconcile_one(&app, &dir)?;

    let record = store
        .get_session(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("сессия {session_id} не найдена"))?;

    let segs = store.get_segments(&session_id).map_err(|e| e.to_string())?;
    let total_frames: u64 = segs.iter().map(|s| s.frames).sum();
    let duration_seconds = if record.sample_rate_hz > 0 {
        total_frames / record.sample_rate_hz as u64
    } else {
        0
    };
    let segments_hashed = segs.iter().filter(|s| !s.sha256.is_empty()).count() as u32;
    let integrity = IntegritySummary {
        session_id: session_id.clone(),
        segments: segs.len() as u32,
        segments_hashed,
        final_chain_link: record.final_chain_link.clone(),
        hash_chain_enabled: settings.integrity.hash_chain,
        event_log_enabled: settings.integrity.event_log,
    };

    let events = store.get_events(&session_id).map_err(|e| e.to_string())?;
    let snapshot = annotations::fold(&store.get_annotations(&session_id).map_err(|e| e.to_string())?);
    let comment = session_comment::read(&PathBuf::from(&record.dir));

    Ok(SessionDetail {
        segment_count: segs.len() as u32,
        duration_seconds,
        integrity,
        events,
        markers: snapshot.markers,
        role_spans: snapshot.role_spans,
        comment,
        record,
    })
}

/// Сохранить/очистить свободный комментарий оператора к сессии (мелочь трения
/// 10.6). Пустой текст удаляет заметку. Пишется файлом в каталоге сессии
/// (`store::session_comment`) — вне контура целостности/выгрузки.
#[tauri::command]
pub fn set_session_comment(app: AppHandle, dir: String, text: String) -> Result<(), String> {
    // Резолвим каталог сессии через манифест (тот же путь, что и карточка) —
    // чтобы не писать в произвольный каталог, а только в начатую сессию.
    let (store, session_id) = open_and_reconcile_one(&app, &dir)?;
    let record = store
        .get_session(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("сессия {session_id} не найдена"))?;
    session_comment::write(&PathBuf::from(&record.dir), &text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::journal::{Journal, JournalRecord};
    use crate::recorder::segment_writer::{SegmentConfig, SegmentWriter};
    use std::time::Duration;

    /// Собрать каталог сессии с настоящим WAV-сегментом и журналом.
    fn build_session(root: &Path, name: &str, stopped: bool) {
        let dir = root.join(name);
        let mut journal = Journal::open(&dir).unwrap();
        journal
            .append(&JournalRecord::SessionStarted {
                started_at_unix_ms: 1_700_000_000_000,
                sample_rate_hz: 8_000,
                channels: 1,
                bit_depth: 16,
                segment_seconds: 30,
                operator_id: String::new(),
                station_id: String::new(),
            })
            .unwrap();
        let cfg = SegmentConfig {
            dir: dir.clone(),
            sample_rate_hz: 8_000,
            channels: 1,
            bits_per_sample: 16,
            segment_seconds: 3_600,
            flush_interval: Duration::from_millis(1_500),
        };
        let mut w = SegmentWriter::new(cfg).unwrap();
        let samples: Vec<i16> = (0..200).map(|i| (i % 50) as i16).collect();
        w.write_samples(&samples).unwrap();
        let segs = w.finalize().unwrap();
        let file_name = segs[0]
            .path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        journal
            .append(&JournalRecord::SegmentCompleted {
                index: 1,
                path: file_name,
                frames: segs[0].frames,
                started_at_unix_ms: segs[0].started_at_unix_ms as u64,
            })
            .unwrap();
        if stopped {
            journal.append(&JournalRecord::Stopped).unwrap();
        }
    }

    #[test]
    fn reconcile_all_picks_up_session_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        build_session(tmp.path(), "session-1", true);
        build_session(tmp.path(), "session-2", false);

        let store = ManifestStore::open(&tmp.path().join(MANIFEST_FILE)).unwrap();
        reconcile_all(&store, tmp.path());

        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        // Целостность зафиксирована: финальное звено цепочки проставлено.
        for s in &sessions {
            assert!(s.final_chain_link.is_some());
            assert_eq!(store.get_segments(&s.id).unwrap().len(), 1);
        }
    }

    #[test]
    fn reconcile_all_on_missing_root_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        let store = ManifestStore::in_memory().unwrap();
        reconcile_all(&store, &missing); // не паникует
        assert!(store.list_sessions().unwrap().is_empty());
    }

    #[test]
    fn disk_status_codes_map() {
        assert_eq!(disk_status_code(DiskStatus::Ok), "ok");
        assert_eq!(disk_status_code(DiskStatus::Low), "low");
        assert_eq!(disk_status_code(DiskStatus::Critical), "critical");
    }

    #[test]
    fn session_detail_pieces_from_reconciled_session() {
        // Путь данных карточки сессии (session_detail): собрать сессию фикстурой,
        // реконсилировать в манифест, затем прочитать события/сегменты/целостность
        // и комментарий — те же вызовы store, что делает команда.
        let tmp = tempfile::tempdir().unwrap();
        build_session(tmp.path(), "session-42", true);
        let dir = tmp.path().join("session-42");

        let store = ManifestStore::open(&tmp.path().join(MANIFEST_FILE)).unwrap();
        let session_id = reconcile::reconcile_session(&store, &dir).unwrap().unwrap();

        let record = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(record.status, crate::store::manifest::SessionStatus::Stopped);

        let segs = store.get_segments(&session_id).unwrap();
        assert_eq!(segs.len(), 1);
        let hashed = segs.iter().filter(|s| !s.sha256.is_empty()).count();
        assert_eq!(hashed, segs.len()); // целостность зафиксирована реконсиляцией

        // Журнал событий содержит старт и стоп.
        let events = store.get_events(&session_id).unwrap();
        assert!(events.iter().any(|e| matches!(
            e.event.kind,
            crate::integrity::events::EventKind::SessionStarted
        )));

        // Комментарий: пусто → задать → прочитать.
        assert_eq!(session_comment::read(&dir), None);
        session_comment::write(&dir, "проверка связи").unwrap();
        assert_eq!(session_comment::read(&dir).as_deref(), Some("проверка связи"));
    }
}
