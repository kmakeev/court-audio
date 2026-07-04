//! Tauri-команды аутентификации оператора (этап 10.3 — `promts/10_3_auth.md`,
//! шаги 2–4).
//!
//! Тонкий слой над клиентом [`crate::sync::auth`] и зашифрованным кэшем
//! [`crate::store::auth_cache`]. Держит **в памяти** активную сессию оператора
//! ([`AuthState`], managed в `lib.rs` как `CaptureState`), гейтит старт записи
//! ([`ensure_start_allowed`]) и отдаёт UI идентичность/статус связи + событие
//! смены состояния (`auth_state`).
//!
//! **Надёжность важнее строгости сессии:** идущая запись не читает `AuthState`
//! (гейт срабатывает только на старте новой сессии), поэтому выход/истечение
//! токена/смена оператора её не прерывают.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::ipc::{load_settings, resolve_storage_root};
use crate::reliability::watchdog::now_unix_ms;
use crate::settings::Settings;
use crate::store::auth_cache;
use crate::sync::auth::{
    cache_expires_at_unix_ms, cached_session_valid, hash_pin, verify_pin, AuthTransport,
    CachedSession, HttpAuthTransport, OperatorProfile,
};

/// Событие смены состояния аутентификации (шапка UI перечитывает статус).
pub const EVENT_AUTH_STATE: &str = "auth_state";

/// Активная сессия оператора в памяти ядра. `access_token = None` — оффлайн-режим
/// (разблокировано по кэшу без свежего access; тихий refresh поднимет онлайн).
#[derive(Debug, Clone)]
pub struct ActiveOperator {
    pub operator_id: String,
    pub full_name: String,
    pub role: String,
    pub access_token: Option<String>,
    pub refresh_token: String,
    pub obtained_at_unix_ms: u64,
    pub online: bool,
}

impl ActiveOperator {
    fn profile(&self) -> OperatorProfile {
        OperatorProfile {
            operator_id: self.operator_id.clone(),
            full_name: self.full_name.clone(),
            role: self.role.clone(),
        }
    }
}

/// Managed-состояние аутентификации (единственный источник «кто вошёл»).
#[derive(Default)]
pub struct AuthState(pub Mutex<Option<ActiveOperator>>);

// ── Представления для UI ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct OperatorView {
    pub operator_id: String,
    pub full_name: String,
    pub role: String,
}

impl From<OperatorProfile> for OperatorView {
    fn from(p: OperatorProfile) -> Self {
        Self {
            operator_id: p.operator_id,
            full_name: p.full_name,
            role: p.role,
        }
    }
}

/// Снимок состояния аутентификации для UI (шапка + гейт + экран входа).
#[derive(Debug, Clone, Serialize)]
pub struct AuthStatusView {
    /// Вошедший оператор (или `null`, если входа ещё не было).
    pub operator: Option<OperatorView>,
    /// Есть ли связь с сервером (онлайн-сессия vs оффлайн-разблокировка).
    pub online: bool,
    /// Доступен ли оффлайн-старт по кэшу (валидный кэш в окне), пока не вошли.
    pub offline_cached: bool,
    /// Момент истечения кэша (unix ms) — для UI-индикации окна оффлайн-старта.
    pub cache_expires_at_unix_ms: Option<u64>,
    /// Требуется ли PIN для оффлайн-разблокировки (`auth.operator.offline_pin`).
    pub pin_required: bool,
}

// ── Чистая логика (тестируемая без Tauri) ─────────────────────────────────────

/// Разрешён ли старт записи: при `required_to_start` нужен активный оператор.
pub fn start_allowed(required_to_start: bool, operator_present: bool) -> bool {
    !required_to_start || operator_present
}

/// Fail-secure решение о старте по **результату загрузки настроек** (R-003,
/// этап 13.5). Повреждённый конфиг (`Err`) НИКОГДА не размыкает гейт: старт
/// запрещён с явной диагностикой, а не разрешён «в открытом режиме» на
/// неизвестной политике. Чистая функция (тестируется без Tauri).
pub fn start_gate_decision(
    config: Result<&Settings, &str>,
    operator_present: bool,
) -> Result<(), String> {
    match config {
        Ok(settings) => {
            if start_allowed(settings.auth.operator.required_to_start, operator_present) {
                Ok(())
            } else {
                Err("Требуется вход оператора: авторизуйтесь перед началом записи".to_string())
            }
        }
        // Битый `settings.json` → смыкаем гейт (не пускаем старт).
        Err(_) => Err(crate::ipc::CONFIG_CORRUPT_MESSAGE.to_string()),
    }
}

/// Ошибка оффлайн-разблокировки по кэшу (понятная для UI).
#[derive(Debug, PartialEq, Eq)]
pub enum OfflineUnlockError {
    /// Окно `cached_session_hours` истекло — нужен онлайн-вход.
    Expired,
    /// Неверный PIN.
    WrongPin,
    /// Кэш отсутствует.
    NoCache,
}

impl std::fmt::Display for OfflineUnlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OfflineUnlockError::Expired => write!(
                f,
                "срок кэшированной сессии истёк — требуется вход с подключением к серверу"
            ),
            OfflineUnlockError::WrongPin => write!(f, "неверный PIN"),
            OfflineUnlockError::NoCache => write!(
                f,
                "нет кэшированной сессии — требуется вход с подключением к серверу"
            ),
        }
    }
}

/// Решение об оффлайн-разблокировке: сначала окно кэша, затем PIN (если требуется).
pub fn offline_unlock_decision(
    session: &CachedSession,
    now_unix_ms: u64,
    cached_session_hours: u32,
    pin: &str,
    pin_required: bool,
) -> Result<(), OfflineUnlockError> {
    if !cached_session_valid(session.obtained_at_unix_ms, now_unix_ms, cached_session_hours) {
        return Err(OfflineUnlockError::Expired);
    }
    if pin_required && !verify_pin(pin, &session.pin_salt, &session.pin_hash) {
        return Err(OfflineUnlockError::WrongPin);
    }
    Ok(())
}

// ── Помощники доступа к идентичности (для audio/sync/case) ─────────────────────

/// `operator_id` вошедшего оператора (или пусто). Боевой источник идентичности
/// разметки/сессий/аудита с этапа 10.3.
pub(crate) fn current_operator_id(app: &AppHandle) -> String {
    app.try_state::<AuthState>()
        .and_then(|s| s.0.lock().ok().and_then(|g| g.as_ref().map(|o| o.operator_id.clone())))
        .unwrap_or_default()
}

/// Access-токен вошедшего оператора для выгрузки (или `None` → очередь копится).
/// Боевой код токен из env **не читает** (снята подпорка `OPERATOR_TOKEN_ENV`).
pub(crate) fn current_access_token(app: &AppHandle) -> Option<String> {
    app.try_state::<AuthState>()
        .and_then(|s| s.0.lock().ok().and_then(|g| g.as_ref().and_then(|o| o.access_token.clone())))
}

/// Идентичность станции (учётка транспорта выгрузки). Отдельный от операторского
/// входа контур: своего экрана в v1 нет, поэтому источник — env-seam станции
/// (как парольная фраза ключа). Числовой/строковый `station_id` уходит в манифест
/// и `SessionMeta`.
pub(crate) fn station_identity() -> String {
    std::env::var(crate::sync::STATION_ID_ENV)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_default()
}

/// Гейт старта записи: при `auth.operator.required_to_start` без активного
/// оператора — понятный отказ. Используется `start_capture`.
pub fn ensure_start_allowed(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let present = app
        .try_state::<AuthState>()
        .map(|s| s.0.lock().map(|g| g.is_some()).unwrap_or(false))
        .unwrap_or(false);
    // Сюда `settings` доходят уже разобранными (битый конфиг отсекает
    // `load_settings` выше по стеку тем же fail-secure сообщением); гейт-логика
    // едина с чистой [`start_gate_decision`].
    start_gate_decision(Ok(settings), present)
}

// ── Общие помощники команд ────────────────────────────────────────────────────

fn emit_state(app: &AppHandle, status: &AuthStatusView) {
    let _ = app.emit(EVENT_AUTH_STATE, status);
}

/// Собрать снимок статуса из текущего состояния + (для оффлайна) наличия кэша.
fn status_view(app: &AppHandle, state: &AuthState) -> Result<AuthStatusView, String> {
    let settings = load_settings(app)?;
    let pin_required = settings.auth.operator.offline_pin.required;
    let hours = settings.auth.operator.cached_session_hours;
    let guard = state.0.lock().map_err(|_| "состояние входа повреждено".to_string())?;
    if let Some(op) = guard.as_ref() {
        return Ok(AuthStatusView {
            operator: Some(op.profile().into()),
            online: op.online,
            offline_cached: false,
            cache_expires_at_unix_ms: Some(cache_expires_at_unix_ms(op.obtained_at_unix_ms, hours)),
            pin_required,
        });
    }
    drop(guard);
    // Ещё не вошли: подсказать UI, доступен ли оффлайн-старт по валидному кэшу.
    let (offline_cached, expires) = match load_cached_session(app, &settings) {
        Some(s) if cached_session_valid(s.obtained_at_unix_ms, now_unix_ms(), hours) => {
            (true, Some(cache_expires_at_unix_ms(s.obtained_at_unix_ms, hours)))
        }
        _ => (false, None),
    };
    Ok(AuthStatusView {
        operator: None,
        online: false,
        offline_cached,
        cache_expires_at_unix_ms: expires,
        pin_required,
    })
}

/// Прочитать кэш-сессию (мягко: недоступный ключ/битый блоб → `None`).
fn load_cached_session(app: &AppHandle, settings: &Settings) -> Option<CachedSession> {
    let root = resolve_storage_root(app, settings).ok()?;
    auth_cache::load(&root, settings.storage.key_source).ok().flatten()
}

// ── Команды ───────────────────────────────────────────────────────────────────

/// Вход оператора: логин/пароль → JWT `ex_system`; профиль в шапку; кэш сессии
/// (билет + refresh + хеш PIN) — зашифрованно для оффлайн-старта.
#[tauri::command]
pub fn auth_login(
    app: AppHandle,
    state: State<'_, AuthState>,
    email: String,
    password: String,
    pin: Option<String>,
) -> Result<AuthStatusView, String> {
    let settings = load_settings(&app)?;
    let base_url = settings
        .sync
        .server_base_url
        .clone()
        .ok_or_else(|| "не задан адрес сервера ex_system (настройки → выгрузка)".to_string())?;

    // PIN обязателен для последующего оффлайн-старта — валидируем сразу на входе.
    let pin_cfg = &settings.auth.operator.offline_pin;
    let pin = pin.unwrap_or_default();
    if pin_cfg.required && (pin.trim().len() as u32) < pin_cfg.min_length {
        return Err(format!(
            "PIN должен быть не короче {} символов",
            pin_cfg.min_length
        ));
    }

    let transport = HttpAuthTransport::new(base_url).map_err(|e| e.to_string())?;
    let tokens = transport
        .obtain_token(email.trim(), &password)
        .map_err(|e| e.to_string())?;
    let profile = transport
        .fetch_profile(&tokens.access)
        .map_err(|e| e.to_string())?;

    let obtained_at = now_unix_ms();
    let (pin_salt, pin_hash) = if pin_cfg.required {
        hash_pin(pin.trim())
    } else {
        (Vec::new(), Vec::new())
    };

    // Кэш оффлайн-сессии — **всегда зашифрованно** ключом станции. R-004
    // (этап 13.5): сбой шифрования НЕ проглатывается. Раньше `let _ = save(..)`
    // прятал отсутствие ключа станции: онлайн-вход «успешен», а офлайн-старт по
    // PIN в следующий раз молча недоступен. Теперь неудача доводится до UI явной
    // ошибкой (ключ станции обязателен при развёртывании — см. packaging.md).
    let root = resolve_storage_root(&app, &settings)?;
    let cached = CachedSession {
        operator_id: profile.operator_id.clone(),
        full_name: profile.full_name.clone(),
        role: profile.role.clone(),
        refresh_token: tokens.refresh.clone(),
        obtained_at_unix_ms: obtained_at,
        pin_salt,
        pin_hash,
    };
    auth_cache::save(&root, settings.storage.key_source, &cached).map_err(|e| {
        format!(
            "офлайн-режим недоступен: не удалось сохранить сессию ({e}). \
             Задайте ключ станции ({}) при развёртывании.",
            crate::store::crypto::PASSPHRASE_ENV
        )
    })?;

    {
        let mut guard = state.0.lock().map_err(|_| "состояние входа повреждено".to_string())?;
        *guard = Some(ActiveOperator {
            operator_id: profile.operator_id,
            full_name: profile.full_name,
            role: profile.role,
            access_token: Some(tokens.access),
            refresh_token: tokens.refresh,
            obtained_at_unix_ms: obtained_at,
            online: true,
        });
    }
    let status = status_view(&app, &state)?;
    emit_state(&app, &status);
    Ok(status)
}

/// Оффлайн-разблокировка по кэшу: проверка окна + PIN → активная оффлайн-сессия
/// (без access-токена; тихий refresh поднимет онлайн при возврате связи).
#[tauri::command]
pub fn auth_unlock_offline(
    app: AppHandle,
    state: State<'_, AuthState>,
    pin: Option<String>,
) -> Result<AuthStatusView, String> {
    let settings = load_settings(&app)?;
    let session = load_cached_session(&app, &settings).ok_or_else(|| OfflineUnlockError::NoCache.to_string())?;
    offline_unlock_decision(
        &session,
        now_unix_ms(),
        settings.auth.operator.cached_session_hours,
        pin.as_deref().unwrap_or_default(),
        settings.auth.operator.offline_pin.required,
    )
    .map_err(|e| e.to_string())?;

    {
        let mut guard = state.0.lock().map_err(|_| "состояние входа повреждено".to_string())?;
        *guard = Some(ActiveOperator {
            operator_id: session.operator_id.clone(),
            full_name: session.full_name.clone(),
            role: session.role.clone(),
            access_token: None,
            refresh_token: session.refresh_token.clone(),
            obtained_at_unix_ms: session.obtained_at_unix_ms,
            online: false,
        });
    }
    let status = status_view(&app, &state)?;
    emit_state(&app, &status);
    Ok(status)
}

/// Тихий refresh при возврате онлайн: оффлайн-сессия + refresh-токен → новый
/// access, `online = true`, без действий оператора. Сеть недоступна — тихо
/// остаёмся оффлайн (не ошибка).
#[tauri::command]
pub fn auth_reconnect(
    app: AppHandle,
    state: State<'_, AuthState>,
) -> Result<AuthStatusView, String> {
    try_silent_refresh(&app);
    status_view(&app, &state)
}

/// Внутренний тихий refresh (используется командой и планировщиком выгрузки).
/// Ничего не делает, если уже онлайн, нет refresh-токена или адреса сервера.
pub(crate) fn try_silent_refresh(app: &AppHandle) {
    let Some(state) = app.try_state::<AuthState>() else {
        return;
    };
    let refresh = {
        let guard = match state.0.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        match guard.as_ref() {
            Some(op) if !op.online => op.refresh_token.clone(),
            _ => return,
        }
    };
    let Ok(settings) = load_settings(app) else {
        return;
    };
    let Some(base_url) = settings.sync.server_base_url.clone() else {
        return;
    };
    let Ok(transport) = HttpAuthTransport::new(base_url) else {
        return;
    };
    if let Ok(access) = transport.refresh(&refresh) {
        let mut guard = match state.0.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(op) = guard.as_mut() {
            op.access_token = Some(access);
            op.online = true;
        }
        drop(guard);
        if let Ok(status) = status_view(app, &state) {
            emit_state(app, &status);
        }
    }
}

/// Выход оператора: чистит сессию **в памяти**. Кэш оффлайн-сессии **сохраняется**
/// (зашифрован, ограничен окном `cached_session_hours`), иначе в оффлайн-зале
/// «Сменить оператора» лишил бы станцию возможности снова войти по PIN без связи;
/// кэш перезапишется при следующем онлайн-входе или истечёт по окну.
/// **Идущую запись не трогает** (гейт только на старте новой сессии).
#[tauri::command]
pub fn auth_logout(
    app: AppHandle,
    state: State<'_, AuthState>,
) -> Result<AuthStatusView, String> {
    {
        let mut guard = state.0.lock().map_err(|_| "состояние входа повреждено".to_string())?;
        *guard = None;
    }
    let status = status_view(&app, &state)?;
    emit_state(&app, &status);
    Ok(status)
}

/// Текущий статус аутентификации (шапка/гейт/экран входа читают при монтировании).
#[tauri::command]
pub fn auth_status(
    app: AppHandle,
    state: State<'_, AuthState>,
) -> Result<AuthStatusView, String> {
    status_view(&app, &state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(obtained_at: u64, pin: Option<&str>) -> CachedSession {
        let (pin_salt, pin_hash) = match pin {
            Some(p) => hash_pin(p),
            None => (Vec::new(), Vec::new()),
        };
        CachedSession {
            operator_id: "42".into(),
            full_name: "Иванов И. И.".into(),
            role: "assistant".into(),
            refresh_token: "r".into(),
            obtained_at_unix_ms: obtained_at,
            pin_salt,
            pin_hash,
        }
    }

    #[test]
    fn start_gate_matrix() {
        // required=false → всегда разрешено.
        assert!(start_allowed(false, false));
        assert!(start_allowed(false, true));
        // required=true → только с активным оператором.
        assert!(!start_allowed(true, false));
        assert!(start_allowed(true, true));
    }

    #[test]
    fn offline_unlock_within_window_with_pin() {
        let now = 2_000_000_000_000u64;
        let s = cached(now - 60_000, Some("2468"));
        assert_eq!(offline_unlock_decision(&s, now, 24, "2468", true), Ok(()));
        assert_eq!(
            offline_unlock_decision(&s, now, 24, "0000", true),
            Err(OfflineUnlockError::WrongPin)
        );
    }

    #[test]
    fn offline_unlock_rejected_out_of_window() {
        let now = 2_000_000_000_000u64;
        // Вход был 25ч назад при окне 24ч.
        let s = cached(now - 25 * 60 * 60 * 1000, Some("2468"));
        assert_eq!(
            offline_unlock_decision(&s, now, 24, "2468", true),
            Err(OfflineUnlockError::Expired)
        );
    }

    #[test]
    fn offline_unlock_without_pin_when_not_required() {
        let now = 2_000_000_000_000u64;
        let s = cached(now - 60_000, None);
        // PIN не требуется — любой (в т.ч. пустой) проходит в окне.
        assert_eq!(offline_unlock_decision(&s, now, 24, "", false), Ok(()));
    }
}
