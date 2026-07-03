//! Провижининг и проверка админ-PIN станции (этап 10.4 —
//! `promts/10_4_settings_roles.md`, шаг 1).
//!
//! Права администратора в v1 — **валидный админ-PIN** (оффлайн-фолбэк; работать
//! в оффлайне обязательно, Решено). PIN **задаётся при развёртывании** через env
//! `COURT_AUDIO_ADMIN_PIN` и хранится **как Argon2id-хеш** в зашифрованном блобе
//! `admin_pin.enc` (AES-256-GCM ключом станции через [`super::crypto`]) — как
//! кэш оффлайн-сессии ([`super::auth_cache`]), но несёт только соль/хеш PIN.
//! Секрет в `settings.json` **не хранится**.
//!
//! На первом запуске [`provision_from_env_if_absent`] засеивает хеш из env
//! (env потом можно снять); дальше проверка идёт оффлайн против блоба
//! ([`verify`]). Argon2id/const-time сравнение переиспользуются из
//! [`crate::sync::auth`] — единый крипто-путь PIN на станции.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::crypto;
use super::StoreError;
use crate::settings::KeySource;
use crate::sync::auth::{hash_pin, verify_pin};

/// Env-переменная с админ-PIN станции (секрет, не в `settings.json`). Читается
/// только при провижининге блоба; после — можно снять.
pub const ADMIN_PIN_ENV: &str = "COURT_AUDIO_ADMIN_PIN";

/// Имя зашифрованного файла с хешем админ-PIN в корне хранилища.
pub const ADMIN_PIN_FILE: &str = "admin_pin.enc";

/// Персистируемая запись админ-PIN: соль + Argon2id-хеш (сам PIN не хранится).
/// Кладётся под AES-256-GCM ключом станции.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AdminPinRecord {
    pin_salt: Vec<u8>,
    pin_hash: Vec<u8>,
}

fn pin_path(root: &Path) -> PathBuf {
    root.join(ADMIN_PIN_FILE)
}

/// Задан ли админ-PIN (блоб на месте). Не дешифрует — только наличие файла.
pub fn is_provisioned(root: &Path) -> bool {
    pin_path(root).exists()
}

/// Записать хеш админ-PIN зашифрованно (перезаписывает существующий). Создаёт
/// корень при отсутствии.
pub fn provision(root: &Path, key_source: KeySource, pin: &str) -> Result<(), StoreError> {
    std::fs::create_dir_all(root)?;
    let (pin_salt, pin_hash) = hash_pin(pin);
    let record = AdminPinRecord { pin_salt, pin_hash };
    let json = serde_json::to_vec(&record)?;
    let key = crypto::resolve_station_key(key_source, root)?;
    let blob = crypto::encrypt_bytes(&json, &key)?;
    std::fs::write(pin_path(root), blob)?;
    Ok(())
}

/// Засеять админ-PIN из env `COURT_AUDIO_ADMIN_PIN`, **только если блоб ещё не
/// задан** (идемпотентно между запусками: заданный PIN не перезаписывается).
/// Возвращает `true`, если провижининг произошёл. Пустой env или короче
/// `min_length` — тихо пропускаем (не провижиним «пустой» PIN).
pub fn provision_from_env_if_absent(
    root: &Path,
    key_source: KeySource,
    min_length: u32,
) -> Result<bool, StoreError> {
    if is_provisioned(root) {
        return Ok(false);
    }
    let pin = match std::env::var(ADMIN_PIN_ENV) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let pin = pin.trim();
    if (pin.len() as u32) < min_length {
        return Ok(false);
    }
    provision(root, key_source, pin)?;
    Ok(true)
}

/// Проверить админ-PIN против сохранённого хеша (const-time). Блоб отсутствует →
/// `Ok(false)` (не задан = не подтверждён). Порча блоба/неверный ключ →
/// [`StoreError::Crypto`].
pub fn verify(root: &Path, key_source: KeySource, pin: &str) -> Result<bool, StoreError> {
    let path = pin_path(root);
    if !path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read(&path)?;
    let key = crypto::resolve_station_key(key_source, root)?;
    let json = crypto::decrypt_bytes(&raw, &key)?;
    let record: AdminPinRecord = serde_json::from_slice(&json)?;
    Ok(verify_pin(pin, &record.pin_salt, &record.pin_hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_test_passphrase() {
        std::env::set_var(crypto::PASSPHRASE_ENV, "station-test-secret");
    }

    #[test]
    fn provision_and_verify_roundtrip() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        provision(tmp.path(), KeySource::Passphrase, "8421").unwrap();
        assert!(is_provisioned(tmp.path()));
        assert!(verify(tmp.path(), KeySource::Passphrase, "8421").unwrap());
        assert!(!verify(tmp.path(), KeySource::Passphrase, "0000").unwrap());
    }

    #[test]
    fn blob_does_not_contain_pin_plaintext() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        provision(tmp.path(), KeySource::Passphrase, "135790").unwrap();
        let raw = std::fs::read(pin_path(tmp.path())).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"135790"));
    }

    #[test]
    fn verify_absent_is_false() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_provisioned(tmp.path()));
        assert!(!verify(tmp.path(), KeySource::Passphrase, "1234").unwrap());
    }

    // Один тест на env-seeding: `ADMIN_PIN_ENV` — process-global, поэтому не
    // размазываем его по нескольким параллельным тестам (гонка значения), а
    // держим весь жизненный цикл переменной внутри одного теста.
    #[test]
    fn provision_from_env_seeds_once_and_respects_min_length() {
        set_test_passphrase();

        // Env не задан — пропускаем, блоб не появляется.
        std::env::remove_var(ADMIN_PIN_ENV);
        let empty = tempfile::tempdir().unwrap();
        assert!(!provision_from_env_if_absent(empty.path(), KeySource::Passphrase, 4).unwrap());
        assert!(!is_provisioned(empty.path()));

        // Слишком короткий — не провижиним.
        std::env::set_var(ADMIN_PIN_ENV, "12");
        let short = tempfile::tempdir().unwrap();
        assert!(!provision_from_env_if_absent(short.path(), KeySource::Passphrase, 4).unwrap());
        assert!(!is_provisioned(short.path()));

        // Валидный — засеивает один раз; повтор с другим env не перезаписывает.
        std::env::set_var(ADMIN_PIN_ENV, "246810");
        let tmp = tempfile::tempdir().unwrap();
        assert!(provision_from_env_if_absent(tmp.path(), KeySource::Passphrase, 4).unwrap());
        assert!(verify(tmp.path(), KeySource::Passphrase, "246810").unwrap());

        std::env::set_var(ADMIN_PIN_ENV, "999999");
        assert!(!provision_from_env_if_absent(tmp.path(), KeySource::Passphrase, 4).unwrap());
        assert!(verify(tmp.path(), KeySource::Passphrase, "246810").unwrap());
        assert!(!verify(tmp.path(), KeySource::Passphrase, "999999").unwrap());

        std::env::remove_var(ADMIN_PIN_ENV);
    }
}
