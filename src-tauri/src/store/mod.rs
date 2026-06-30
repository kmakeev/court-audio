//! Локальное хранилище и манифест (этап 03 — `promts/03_store_integrity.md`).
//!
//! SQLite-манифест сессий и сегментов ([`db`] + [`manifest`]) — источник для
//! запросов и UI; аварийно-устойчивый журнал из этапа 02 остаётся «последней
//! инстанцией» при восстановлении и реконсилируется в SQLite ([`reconcile`]).
//! Шифрование сегментов at-rest (AES-256-GCM, [`crypto`]); экспорт манифеста
//! записи в JSON для серверной верификации ([`export`], контракт `07`); движок
//! локального ретеншна ([`retention`]) — зеркало серверного `purge_expired_uploads`.

pub mod crypto;
pub mod db;
pub mod export;
pub mod manifest;
pub mod reconcile;
pub mod retention;

/// Единая ошибка слоя хранилища. Подсистемы заворачивают свои ошибки сюда, чтобы
/// IPC-слой (этап 04) отдавал UI один тип.
#[derive(Debug)]
pub enum StoreError {
    /// Ошибка SQLite-манифеста.
    Db(String),
    /// Ошибка файлового ввода-вывода.
    Io(String),
    /// Ошибка шифрования/ключа at-rest.
    Crypto(String),
    /// Ошибка вычисления хеша/цепочки целостности.
    Hash(String),
    /// Ошибка (де)сериализации (манифест/события).
    Serde(String),
    /// Запрошенной сущности нет в манифесте.
    NotFound(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "ошибка манифеста SQLite: {e}"),
            StoreError::Io(e) => write!(f, "ошибка ввода-вывода хранилища: {e}"),
            StoreError::Crypto(e) => write!(f, "ошибка шифрования at-rest: {e}"),
            StoreError::Hash(e) => write!(f, "ошибка целостности: {e}"),
            StoreError::Serde(e) => write!(f, "ошибка сериализации: {e}"),
            StoreError::NotFound(e) => write!(f, "не найдено в манифесте: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Db(e.to_string())
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Serde(e.to_string())
    }
}

impl From<crate::integrity::hash::HashError> for StoreError {
    fn from(e: crate::integrity::hash::HashError) -> Self {
        StoreError::Hash(e.to_string())
    }
}
