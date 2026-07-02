//! Зашифрованный персист кэш-сессии оператора для оффлайн-старта (этап 10.3 —
//! `promts/10_3_auth.md`, шаг 1).
//!
//! Хранит [`CachedSession`](crate::sync::auth::CachedSession) отдельным
//! **всегда зашифрованным** блоб-файлом (AES-256-GCM ключом станции через
//! [`super::crypto`]) — как кэш дел ([`super::case_cache`]), но без plaintext-
//! варианта: блоб несёт refresh-токен, идентичность и хеш PIN. Логика окна/PIN —
//! в [`crate::sync::auth`]; здесь только сохранение/чтение/очистка.
//!
//! Если ключ станции недоступен (нет passphrase и OS-keystore ещё заглушка),
//! кэш **не пишется** (online-only режим) — это не роняет вход.

use std::path::{Path, PathBuf};

use super::crypto;
use super::StoreError;
use crate::settings::KeySource;
use crate::sync::auth::CachedSession;

/// Имя зашифрованного файла кэш-сессии в корне хранилища.
pub const AUTH_SESSION_FILE: &str = "auth_session.enc";

fn session_path(root: &Path) -> PathBuf {
    root.join(AUTH_SESSION_FILE)
}

/// Сохранить кэш-сессию зашифрованно. Создаёт корень при отсутствии.
pub fn save(
    root: &Path,
    key_source: KeySource,
    session: &CachedSession,
) -> Result<(), StoreError> {
    std::fs::create_dir_all(root)?;
    let json = serde_json::to_vec(session)?;
    let key = crypto::resolve_station_key(key_source, root)?;
    let blob = crypto::encrypt_bytes(&json, &key)?;
    std::fs::write(session_path(root), blob)?;
    Ok(())
}

/// Прочитать кэш-сессию (или `None`, если файла нет). Дешифрует ключом станции;
/// порча блоба/неверный тег → [`StoreError::Crypto`].
pub fn load(root: &Path, key_source: KeySource) -> Result<Option<CachedSession>, StoreError> {
    let path = session_path(root);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read(&path)?;
    let key = crypto::resolve_station_key(key_source, root)?;
    let json = crypto::decrypt_bytes(&raw, &key)?;
    let session: CachedSession = serde_json::from_slice(&json)?;
    Ok(Some(session))
}

/// Удалить кэш-сессию (выход оператора). Отсутствие файла — не ошибка.
pub fn clear(root: &Path) -> Result<(), StoreError> {
    let path = session_path(root);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::auth::hash_pin;

    fn set_test_passphrase() {
        std::env::set_var(crypto::PASSPHRASE_ENV, "station-test-secret");
    }

    fn sample() -> CachedSession {
        let (pin_salt, pin_hash) = hash_pin("2468");
        CachedSession {
            operator_id: "42".into(),
            full_name: "Иванов И. И.".into(),
            role: "assistant".into(),
            refresh_token: "refresh-xyz".into(),
            obtained_at_unix_ms: 1_700_000_000_000,
            pin_salt,
            pin_hash,
        }
    }

    #[test]
    fn save_load_roundtrip() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        let s = sample();
        save(tmp.path(), KeySource::Passphrase, &s).unwrap();
        // Блоб на месте и не содержит refresh-токен в открытом виде.
        let raw = std::fs::read(session_path(tmp.path())).unwrap();
        assert!(!raw.windows(11).any(|w| w == b"refresh-xyz"));
        let back = load(tmp.path(), KeySource::Passphrase).unwrap().unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn load_absent_is_none() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        assert!(load(tmp.path(), KeySource::Passphrase).unwrap().is_none());
    }

    #[test]
    fn clear_removes_file() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        save(tmp.path(), KeySource::Passphrase, &sample()).unwrap();
        assert!(session_path(tmp.path()).exists());
        clear(tmp.path()).unwrap();
        assert!(!session_path(tmp.path()).exists());
        // Повторная очистка отсутствующего файла — не ошибка.
        clear(tmp.path()).unwrap();
    }

    #[test]
    fn tampered_blob_fails() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        save(tmp.path(), KeySource::Passphrase, &sample()).unwrap();
        let path = session_path(tmp.path());
        let mut blob = std::fs::read(&path).unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        std::fs::write(&path, &blob).unwrap();
        assert!(matches!(
            load(tmp.path(), KeySource::Passphrase),
            Err(StoreError::Crypto(_))
        ));
    }
}
