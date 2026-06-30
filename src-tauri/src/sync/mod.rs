//! Агент выгрузки записей в `ex_system` (этап 06 — `promts/06_sync_agent.md`).
//!
//! Клиентская сторона возобновляемой чанковой выгрузки по контракту `07`:
//! `sessions/` → `upload/init` → `part/<n>`×N → `complete` → `verify`, с
//! оффлайн-очередью ([`queue`]), ретраями/бэкоффом ([`backoff_delay`] +
//! [`scheduler`]) и **сигналом ретеншну** ([`verify`] → [`crate::store::retention`]).
//!
//! **Приоритет — захват, не выгрузка.** Сетевой агент фоновый: всё сетевое
//! спрятано за trait-seam [`client::UploadTransport`] (как кэш дел этапа 05),
//! поэтому логика тестируется оффлайн на фейк-транспорте; реальный
//! `reqwest`-клиент ([`client::HttpTransport`]) — тонкая обёртка-проводка.
//!
//! Все параметры — из [`crate::settings`] (реестр `docs/configuration.md`,
//! разделы «Выгрузка и сеть», «Аутентификация»); магических чисел нет.

pub mod client;
pub mod queue;
pub mod scheduler;
pub mod uploader;
pub mod verify;

#[cfg(test)]
pub mod testkit;

use std::time::Duration;

use crate::store::StoreError;
use client::{ErrorKind, TransportError};

/// Env-переменная с операторским JWT (транспорт выгрузки). Временный источник
/// токена до этапа `auth` (как `COURT_AUDIO_STATION_PASSPHRASE` для ключа): не
/// настройка-тюнинг, а seam идентичности — секрет в settings.json не хранится.
pub const OPERATOR_TOKEN_ENV: &str = "COURT_AUDIO_OPERATOR_TOKEN";

/// Env-переменные идентичности станции/оператора — временный источник до
/// экрана входа оператора (login-UI). Запись сессии до входа заводится с
/// пустыми `station_id`/`operator_id` (reconcile), а сервер `07` требует
/// **числовой** `operator_id` (PK пользователя ex_system). Эти переменные
/// заполняют пустые значения при регистрации сессии (`upload_session`).
pub const OPERATOR_ID_ENV: &str = "COURT_AUDIO_OPERATOR_ID";
pub const STATION_ID_ENV: &str = "COURT_AUDIO_STATION_ID";

/// Ошибка агента выгрузки. Делит сетевые сбои на временные (ретраить) и
/// постоянные (4xx — не ретраить), плюс отсутствие токена и ошибки стора.
#[derive(Debug)]
pub enum SyncError {
    /// Временная ошибка (сеть/таймаут/5xx) — запись остаётся в очереди, ретрай.
    Transient(String),
    /// Постоянная ошибка (4xx/некорректные данные) — без бесконечного ретрая.
    Permanent(String),
    /// Нет валидного токена — копим в очереди, **не теряем** данные.
    NoToken,
    /// Ошибка локального стора (манифест/чтение сегмента).
    Store(StoreError),
}

impl SyncError {
    /// Временная ли ошибка (стор-ошибки трактуем как временные — диск/манифест
    /// могут восстановиться, данные не теряем).
    pub fn is_transient(&self) -> bool {
        matches!(self, SyncError::Transient(_) | SyncError::Store(_))
    }
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Transient(e) => write!(f, "временная ошибка выгрузки: {e}"),
            SyncError::Permanent(e) => write!(f, "постоянная ошибка выгрузки: {e}"),
            SyncError::NoToken => write!(f, "нет операторского токена для выгрузки"),
            SyncError::Store(e) => write!(f, "ошибка хранилища при выгрузке: {e}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<StoreError> for SyncError {
    fn from(e: StoreError) -> Self {
        SyncError::Store(e)
    }
}

impl From<TransportError> for SyncError {
    fn from(e: TransportError) -> Self {
        match e.kind {
            ErrorKind::Transient => SyncError::Transient(e.msg),
            ErrorKind::Permanent => SyncError::Permanent(e.msg),
        }
    }
}

/// Задержка экспоненциального бэкоффа перед попыткой `attempt` (нумерация с 1):
/// `base · 2^(attempt-1)`, ограниченная `max`. Параметры — из
/// `Settings.sync.retry.*` (без «магических чисел»). Чистая функция (тест).
pub fn backoff_delay(attempt: u32, base_ms: u32, max_ms: u32) -> Duration {
    let base = base_ms as u64;
    let max = max_ms as u64;
    // attempt=1 → base; attempt=2 → base·2; … Насыщение, чтобы сдвиг не паниковал.
    let shift = attempt.saturating_sub(1).min(31);
    let scaled = base.saturating_mul(1u64 << shift);
    Duration::from_millis(scaled.min(max))
}

/// Исчерпан ли лимит попыток. `max_attempts = 0` означает «без лимита, до
/// успеха» (`Settings.sync.retry.max_attempts`).
pub fn attempts_exhausted(attempts: u32, max_attempts: u32) -> bool {
    max_attempts != 0 && attempts >= max_attempts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_exponentially_and_caps() {
        // base=2000, max=60000 (дефолты реестра).
        assert_eq!(
            backoff_delay(1, 2_000, 60_000),
            Duration::from_millis(2_000)
        );
        assert_eq!(
            backoff_delay(2, 2_000, 60_000),
            Duration::from_millis(4_000)
        );
        assert_eq!(
            backoff_delay(3, 2_000, 60_000),
            Duration::from_millis(8_000)
        );
        assert_eq!(
            backoff_delay(4, 2_000, 60_000),
            Duration::from_millis(16_000)
        );
        // Дальше упирается в потолок.
        assert_eq!(
            backoff_delay(10, 2_000, 60_000),
            Duration::from_millis(60_000)
        );
        // Очень большой attempt не паникует (сдвиг насыщается).
        assert_eq!(
            backoff_delay(1_000, 2_000, 60_000),
            Duration::from_millis(60_000)
        );
    }

    #[test]
    fn attempts_limit_semantics() {
        // 0 = без лимита.
        assert!(!attempts_exhausted(1_000, 0));
        // Лимит 3: исчерпан на 3-й.
        assert!(!attempts_exhausted(2, 3));
        assert!(attempts_exhausted(3, 3));
        assert!(attempts_exhausted(4, 3));
    }

    #[test]
    fn transient_classification() {
        assert!(SyncError::Transient("x".into()).is_transient());
        assert!(SyncError::Store(StoreError::Io("x".into())).is_transient());
        assert!(!SyncError::Permanent("x".into()).is_transient());
        assert!(!SyncError::NoToken.is_transient());
    }

    #[test]
    fn transport_error_maps_to_sync_error() {
        let t: SyncError = TransportError {
            kind: ErrorKind::Transient,
            msg: "5xx".into(),
        }
        .into();
        assert!(matches!(t, SyncError::Transient(_)));
        let p: SyncError = TransportError {
            kind: ErrorKind::Permanent,
            msg: "400".into(),
        }
        .into();
        assert!(matches!(p, SyncError::Permanent(_)));
    }
}
