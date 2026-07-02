//! Экспорт записей: папка/DVD, автономное прослушивание (этап 10.2 —
//! `promts/10_2_export.md`). Опирается на движок дешифровки/склейки этапа
//! `10.1` (`crate::player`), манифест/целостность (`03`/`09`), разметку
//! (`10`) — но **не переиспользует** потоковый декодер `player::source`
//! напрямую: тот нормализует семплы в `f32` (лосси для плеера, недопустимо
//! для побайтовой копии) и тихо обрывает поток на повреждённом сегменте
//! (нормально для живого прослушивания, недопустимо для выдаваемой копии).
//! Здесь — отдельный целочисленный путь ([`audio`]) с жёстким отказом на
//! ошибке.
//!
//! Экспорт выводит ПДн из зашифрованного хранилища — управляемое (настройка
//! администратора, `Settings.export.policy`) и всегда журналируемое действие
//! ([`audit`]).

pub mod audio;
pub mod audit;
pub mod dvd;
pub mod flac;
pub mod html;
pub mod manifest;
pub mod package;

use crate::settings::ExportPolicy;

/// Ошибка ядра экспорта.
#[derive(Debug)]
pub enum ExportError {
    /// Не удалось разобрать/собрать аудио (WAV/FLAC).
    Decode(String),
    /// Ошибка ключа/шифрования at-rest.
    Crypto(String),
    /// Ошибка ввода-вывода (файлы пакета).
    Io(String),
    /// Ошибка (де)сериализации (манифест/HTML-данные).
    Serde(String),
    /// Ошибка стора (SQLite-манифест сессии).
    Store(crate::store::StoreError),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Decode(e) => write!(f, "ошибка дешифровки/кодирования аудио: {e}"),
            ExportError::Crypto(e) => write!(f, "ошибка дешифровки сегмента: {e}"),
            ExportError::Io(e) => write!(f, "ошибка ввода-вывода: {e}"),
            ExportError::Serde(e) => write!(f, "ошибка сериализации: {e}"),
            ExportError::Store(e) => write!(f, "ошибка манифеста сессии: {e}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<crate::store::crypto::CryptoError> for ExportError {
    fn from(e: crate::store::crypto::CryptoError) -> Self {
        ExportError::Crypto(e.to_string())
    }
}

impl From<crate::store::StoreError> for ExportError {
    fn from(e: crate::store::StoreError) -> Self {
        ExportError::Store(e)
    }
}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        ExportError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for ExportError {
    fn from(e: serde_json::Error) -> Self {
        ExportError::Serde(e.to_string())
    }
}

/// Итог проверки политики экспорта (`Settings.export.policy`) для попытки с
/// заданным флагом подтверждения оператора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// Экспорт разрешён — можно собирать пакет.
    Allowed,
    /// Экспорт запрещён администратором — попытка журналируется как отказ.
    Denied,
    /// Экспорт разрешён только с подтверждением оператора, а его ещё не было
    /// (обычный шаг мастера, не попытка обхода — не журналируется).
    NeedsConfirmation,
}

/// Проверить политику экспорта. Чистая функция без Tauri/IO-зависимостей.
pub fn check_policy(policy: ExportPolicy, confirmed: bool) -> PermissionOutcome {
    match policy {
        ExportPolicy::Forbidden => PermissionOutcome::Denied,
        ExportPolicy::Allowed => PermissionOutcome::Allowed,
        ExportPolicy::RequiresConfirmation => {
            if confirmed {
                PermissionOutcome::Allowed
            } else {
                PermissionOutcome::NeedsConfirmation
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_always_denies_regardless_of_confirmation() {
        assert_eq!(
            check_policy(ExportPolicy::Forbidden, false),
            PermissionOutcome::Denied
        );
        assert_eq!(
            check_policy(ExportPolicy::Forbidden, true),
            PermissionOutcome::Denied
        );
    }

    #[test]
    fn allowed_is_always_allowed() {
        assert_eq!(
            check_policy(ExportPolicy::Allowed, false),
            PermissionOutcome::Allowed
        );
        assert_eq!(
            check_policy(ExportPolicy::Allowed, true),
            PermissionOutcome::Allowed
        );
    }

    #[test]
    fn requires_confirmation_without_flag_needs_confirmation() {
        assert_eq!(
            check_policy(ExportPolicy::RequiresConfirmation, false),
            PermissionOutcome::NeedsConfirmation
        );
    }

    #[test]
    fn requires_confirmation_with_flag_is_allowed() {
        assert_eq!(
            check_policy(ExportPolicy::RequiresConfirmation, true),
            PermissionOutcome::Allowed
        );
    }
}
