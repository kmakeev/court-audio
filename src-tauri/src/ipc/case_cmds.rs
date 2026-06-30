//! Tauri-команды привязки записи к делу (этап 05 —
//! `promts/05_case_binding_offline.md`, deliverable 3–5).
//!
//! Тонкий слой над ядром: кэш дел ([`crate::store::case_cache`]) и модель
//! привязки ([`crate::store::case_binding`]). Бизнес-логики тут нет — команды
//! лишь резолвят пути/настройки (как [`query_cmds`](super::query_cmds)) и
//! вызывают уже протестированные функции стора. UI получает один тип ошибки
//! (`String`).
//!
//! **Транспорт синхронизации** — HTTP к slim-эндпоинту докета `ex_system`
//! (`GET /audio/docket/`, этап `07`) под станционным JWT (`06`). Реализован в
//! [`sync_case_cache`] через [`docket::DocketHttpFetcher`]; фетчер-агностичная
//! логика кэша ([`case_cache::sync_into_cache`] + [`case_cache::save`]) — на
//! месте и протестирована.

use serde::Serialize;
use tauri::AppHandle;

use crate::ipc::MANIFEST_FILE;
use crate::ipc::{load_settings, resolve_storage_root};
use crate::reliability::watchdog::now_unix_ms;
use crate::store::case_binding::AdjudicationRef;
use crate::store::case_cache::{self, CaseRecord};
use crate::store::manifest::ManifestStore;
use crate::store::reconcile;
use crate::sync::docket::DocketHttpFetcher;
use crate::sync::OPERATOR_TOKEN_ENV;

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

/// Синхронизировать кэш дел из докета `ex_system` (`GET /audio/docket/`).
///
/// Требует адрес сервера (`sync.server_base_url`) и операторский токен
/// (env `COURT_AUDIO_OPERATOR_TOKEN` — до экрана входа). Тянет докет в скоупе
/// станции (сервер определяет его по идентичности станции), режет до
/// `case_cache.max_records` (минимизация ПДн) и сохраняет в **зашифрованный**
/// локальный кэш. Сеть/токен отсутствуют → читаемая ошибка, кэш не трогаем.
#[tauri::command]
pub fn sync_case_cache(app: AppHandle) -> Result<CaseCacheStatus, String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;
    let cc = &settings.case_cache;

    if !cc.enabled {
        return Err("кэш дел отключён в настройках (case_cache.enabled = false)".into());
    }
    let base_url = settings
        .sync
        .server_base_url
        .clone()
        .ok_or("не задан адрес сервера ex_system (sync.server_base_url)")?;
    let token = std::env::var(OPERATOR_TOKEN_ENV)
        .ok()
        .filter(|t| !t.is_empty())
        .ok_or("нет операторского токена (COURT_AUDIO_OPERATOR_TOKEN) — выполните вход оператора")?;

    let fetcher = DocketHttpFetcher::new(base_url, token)?;
    let cache = case_cache::sync_into_cache(&fetcher, &cc.scope, cc.max_records, now_unix_ms())
        .map_err(|e| e.to_string())?;
    case_cache::save(&root, cc, settings.storage.key_source, &cache).map_err(|e| e.to_string())?;

    Ok(CaseCacheStatus {
        is_fresh: cache.is_fresh(cc.ttl_hours, now_unix_ms()),
        record_count: cache.records.len() as u32,
        scope: cache.scope.clone(),
        synced_at_unix_ms: Some(cache.synced_at_unix_ms),
    })
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
