//! Tauri-команды привязки записи к делу (этап 05 —
//! `promts/05_case_binding_offline.md`, deliverable 3–5).
//!
//! Тонкий слой над ядром: кэш дел ([`crate::store::case_cache`]) и модель
//! привязки ([`crate::store::case_binding`]). Бизнес-логики тут нет — команды
//! лишь резолвят пути/настройки (как [`query_cmds`](super::query_cmds)) и
//! вызывают уже протестированные функции стора. UI получает один тип ошибки
//! (`String`).
//!
//! **Транспорт синхронизации** (HTTP к slim-эндпоинту докета `ex_system`)
//! появляется в этапах `06`/`07` (сеть + операторская авторизация + серверный
//! эндпоинт). До тех пор [`sync_case_cache`] фетчера не имеет и честно сообщает
//! об этом; вся фетчер-агностичная логика кэша ([`case_cache::sync_into_cache`])
//! уже на месте и протестирована оффлайн.

use serde::Serialize;
use tauri::AppHandle;

use crate::ipc::MANIFEST_FILE;
use crate::ipc::{load_settings, resolve_storage_root};
use crate::reliability::watchdog::now_unix_ms;
use crate::store::case_binding::AdjudicationRef;
use crate::store::case_cache::{self, CaseRecord};
use crate::store::manifest::ManifestStore;
use crate::store::reconcile;

/// Свежесть кэша дел для UI (индикатор «синхронизировано/устарел»).
#[derive(Debug, Clone, Serialize)]
pub struct CaseCacheStatus {
    /// Когда кэш синхронизирован (unix ms); `None` — кэша ещё нет.
    pub synced_at_unix_ms: Option<u64>,
    /// Свеж ли кэш по `case_cache.ttl_hours`.
    pub is_fresh: bool,
    /// Сколько дел в кэше.
    pub record_count: u32,
    /// Скоуп докета (`case_cache.scope`).
    pub scope: String,
}

/// Текущая свежесть и объём кэша дел (для шапки пикера на экране «Запись»).
#[tauri::command]
pub fn get_case_cache_status(app: AppHandle) -> Result<CaseCacheStatus, String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;
    let cc = &settings.case_cache;

    match case_cache::load(&root, cc, settings.storage.key_source).map_err(|e| e.to_string())? {
        Some(cache) => Ok(CaseCacheStatus {
            is_fresh: cache.is_fresh(cc.ttl_hours, now_unix_ms()),
            record_count: cache.records.len() as u32,
            scope: cache.scope.clone(),
            synced_at_unix_ms: Some(cache.synced_at_unix_ms),
        }),
        None => Ok(CaseCacheStatus {
            synced_at_unix_ms: None,
            is_fresh: false,
            record_count: 0,
            scope: cc.scope.clone(),
        }),
    }
}

/// Оффлайн-поиск дел в локальном кэше (автокомплит пикера). Пустой запрос —
/// весь кэш; отсутствие кэша — пустой список (UI предложит ручной ввод).
#[tauri::command]
pub fn search_cases(app: AppHandle, query: String) -> Result<Vec<CaseRecord>, String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;
    let cc = &settings.case_cache;

    let cache =
        case_cache::load(&root, cc, settings.storage.key_source).map_err(|e| e.to_string())?;
    Ok(cache.map(|c| c.search(&query)).unwrap_or_default())
}

/// Синхронизировать кэш дел из докета `ex_system`.
///
/// В этапе `05` транспорт отсутствует (HTTP-клиент — `06`, slim-эндпоинт и
/// операторская авторизация — `07`), поэтому команда честно сообщает, что
/// синхронизация появится позже. Логика наполнения кэша
/// ([`case_cache::sync_into_cache`] + [`case_cache::save`]) уже готова и будет
/// подключена к реальному фетчеру в `06`.
#[tauri::command]
pub fn sync_case_cache(_app: AppHandle) -> Result<CaseCacheStatus, String> {
    Err(
        "синхронизация кэша дел появится с сетевым агентом (этап 06) и серверным \
         докет-эндпоинтом (этап 07); пока доступны оффлайн-поиск по кэшу и ручной ввод"
            .to_string(),
    )
}

/// Привязать (или уточнить/снять) дело у записи в каталоге `dir`.
///
/// Сначала реконсилируем сессию из её журнала ([`reconcile::reconcile_session`])
/// — это гарантирует строку в манифесте и **сохраняет** прочие поля; затем
/// пишем привязку JSON-строкой в `adjudication_ref`. `binding = None` снимает
/// привязку. Сигнатура по `dir` (как `recover_session`) — переиспользуемо и для
/// экрана «Сессии».
#[tauri::command]
pub fn bind_session_case(
    app: AppHandle,
    dir: String,
    binding: Option<AdjudicationRef>,
) -> Result<(), String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;
    let store = ManifestStore::open(&root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;

    let dir_path = std::path::PathBuf::from(&dir);
    let session_id = reconcile::reconcile_session(&store, &dir_path)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("каталог не содержит начатой сессии: {dir}"))?;

    let json = match binding {
        Some(b) => {
            b.validate().map_err(|e| e.to_string())?;
            Some(b.to_json().map_err(|e| e.to_string())?)
        }
        None => None,
    };
    store
        .set_adjudication_ref(&session_id, json.as_deref())
        .map_err(|e| e.to_string())
}
