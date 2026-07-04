//! Провижининг операторского профиля зала для **автономного офлайн-старта** по
//! PIN (этап 13.6 — B-001, `promts/13_6_offline_pin_and_overlay.md`).
//!
//! В изолированном зале, где `ex_system` недоступен, станцию нельзя запустить
//! впервые: нет ни кэша онлайн-сессии, ни возможности войти. Решение заказчика
//! (см. «Решено» промта): распространить паттерн **админ-PIN**
//! ([`super::admin_pin`]) на **операторскую идентичность**. Отличие от админ-PIN:
//! профиль несёт `operator_id`/`station_id`/ФИО/роль (админ-PIN идентичность не
//! несёт) — источник идентичности при автономном старте.
//!
//! Профиль **задаётся при развёртывании** через env (рядом с
//! `COURT_AUDIO_ADMIN_PIN`, см. `docs/packaging.md`) и хранится **как Argon2id-
//! хеш PIN + профиль** в зашифрованном блобе `operator_profile.enc` (AES-256-GCM
//! ключом станции через [`super::crypto`], та же fail-secure реакция на
//! отсутствие ключа, что у кэш-сессии/админ-PIN, R-004/этап 13.5). Секрет PIN в
//! `settings.json` **не хранится**; сам PIN — только хешем. На первом запуске
//! [`provision_from_env_if_absent`] засеивает профиль из env (env потом можно
//! снять); дальше проверка идёт **оффлайн** против блоба ([`verify`]).
//!
//! **Только для явно провижиненных изолированных залов:** засев вызывается лишь
//! при `auth.operator.autonomous_offline.enabled = true` (гейт — в `lib.rs`);
//! обычные станции профиль не создают и вход не ослабляют.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::crypto;
use super::StoreError;
use crate::settings::KeySource;
use crate::sync::auth::{hash_pin, verify_pin, OperatorProfile};

/// Env с PIN операторского профиля (секрет, не в `settings.json`). Читается
/// только при провижининге блоба; после — можно снять.
pub const OPERATOR_PIN_ENV: &str = "COURT_AUDIO_OPERATOR_PIN";
/// Env с ФИО оператора зала (для шапки/идентичности).
pub const OPERATOR_NAME_ENV: &str = "COURT_AUDIO_OPERATOR_NAME";
/// Env с ролью оператора зала.
pub const OPERATOR_ROLE_ENV: &str = "COURT_AUDIO_OPERATOR_ROLE";

/// Имя зашифрованного файла операторского профиля в корне хранилища.
pub const OPERATOR_PROFILE_FILE: &str = "operator_profile.enc";

/// Персистируемый операторский профиль: идентичность зала + соль/хеш PIN (сам
/// PIN не хранится). Кладётся под AES-256-GCM ключом станции.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionedOperator {
    /// `operator_id` (обязателен в контракте `07`; в автономном зале —
    /// провижиненный, а не PK онлайн-входа).
    pub operator_id: String,
    /// `station_id` — учётка станции/зала (транспорт выгрузки).
    pub station_id: String,
    pub full_name: String,
    pub role: String,
    pin_salt: Vec<u8>,
    pin_hash: Vec<u8>,
}

impl ProvisionedOperator {
    /// Профиль оператора (для UI/идентичности) — без секрета PIN.
    pub fn profile(&self) -> OperatorProfile {
        OperatorProfile {
            operator_id: self.operator_id.clone(),
            full_name: self.full_name.clone(),
            role: self.role.clone(),
        }
    }
}

/// Поля идентичности профиля (без PIN) — вход провижининга.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileIdentity {
    pub operator_id: String,
    pub station_id: String,
    pub full_name: String,
    pub role: String,
}

fn profile_path(root: &Path) -> PathBuf {
    root.join(OPERATOR_PROFILE_FILE)
}

/// Задан ли операторский профиль (блоб на месте). Не дешифрует — только наличие.
pub fn is_provisioned(root: &Path) -> bool {
    profile_path(root).exists()
}

/// Записать профиль зашифрованно (перезаписывает существующий). Создаёт корень.
pub fn provision(
    root: &Path,
    key_source: KeySource,
    identity: &ProfileIdentity,
    pin: &str,
) -> Result<(), StoreError> {
    std::fs::create_dir_all(root)?;
    let (pin_salt, pin_hash) = hash_pin(pin);
    let record = ProvisionedOperator {
        operator_id: identity.operator_id.clone(),
        station_id: identity.station_id.clone(),
        full_name: identity.full_name.clone(),
        role: identity.role.clone(),
        pin_salt,
        pin_hash,
    };
    let json = serde_json::to_vec(&record)?;
    let key = crypto::resolve_station_key(key_source, root)?;
    let blob = crypto::encrypt_bytes(&json, &key)?;
    std::fs::write(profile_path(root), blob)?;
    Ok(())
}

/// Засеять операторский профиль из env, **только если блоб ещё не задан**
/// (идемпотентно между запусками). Идентичность — env
/// `COURT_AUDIO_OPERATOR_ID`/`COURT_AUDIO_STATION_ID` (сеймы станции из
/// `sync::mod`) + `COURT_AUDIO_OPERATOR_NAME`/`_ROLE`; PIN —
/// `COURT_AUDIO_OPERATOR_PIN`. Возвращает `true`, если провижининг произошёл.
/// Нет PIN/`operator_id`/`station_id` или PIN короче `min_length` — тихо
/// пропускаем (не провижиним неполный профиль).
pub fn provision_from_env_if_absent(
    root: &Path,
    key_source: KeySource,
    min_length: u32,
) -> Result<bool, StoreError> {
    if is_provisioned(root) {
        return Ok(false);
    }
    let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string());
    let (Some(pin), Some(operator_id), Some(station_id)) = (
        env(OPERATOR_PIN_ENV),
        env(crate::sync::OPERATOR_ID_ENV),
        env(crate::sync::STATION_ID_ENV),
    ) else {
        return Ok(false);
    };
    if pin.is_empty() || operator_id.is_empty() || station_id.is_empty() {
        return Ok(false);
    }
    if (pin.len() as u32) < min_length {
        return Ok(false);
    }
    let identity = ProfileIdentity {
        operator_id,
        station_id,
        full_name: env(OPERATOR_NAME_ENV).unwrap_or_default(),
        role: env(OPERATOR_ROLE_ENV).unwrap_or_default(),
    };
    provision(root, key_source, &identity, &pin)?;
    Ok(true)
}

/// Прочитать профиль (или `None`, если файла нет). Дешифрует ключом станции;
/// порча блоба/неверный ключ → [`StoreError::Crypto`] (fail-secure: не «пусто»).
pub fn load(root: &Path, key_source: KeySource) -> Result<Option<ProvisionedOperator>, StoreError> {
    let path = profile_path(root);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read(&path)?;
    let key = crypto::resolve_station_key(key_source, root)?;
    let json = crypto::decrypt_bytes(&raw, &key)?;
    let record: ProvisionedOperator = serde_json::from_slice(&json)?;
    Ok(Some(record))
}

/// Проверить PIN против сохранённого хеша (const-time). Блоб отсутствует →
/// `Ok(false)`. Порча блоба/неверный ключ → [`StoreError::Crypto`] (fail-secure).
pub fn verify(root: &Path, key_source: KeySource, pin: &str) -> Result<bool, StoreError> {
    match load(root, key_source)? {
        Some(record) => Ok(verify_pin(pin, &record.pin_salt, &record.pin_hash)),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_test_passphrase() {
        std::env::set_var(crypto::PASSPHRASE_ENV, "station-test-secret");
    }

    fn identity() -> ProfileIdentity {
        ProfileIdentity {
            operator_id: "zal-7".into(),
            station_id: "station-A".into(),
            full_name: "Дежурный оператор зала".into(),
            role: "clerk".into(),
        }
    }

    #[test]
    fn provision_and_verify_roundtrip() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        provision(tmp.path(), KeySource::Passphrase, &identity(), "8421").unwrap();
        assert!(is_provisioned(tmp.path()));
        // Верный PIN проходит, неверный — нет.
        assert!(verify(tmp.path(), KeySource::Passphrase, "8421").unwrap());
        assert!(!verify(tmp.path(), KeySource::Passphrase, "0000").unwrap());
        // Идентичность читается из блоба (источник для автономного старта).
        let rec = load(tmp.path(), KeySource::Passphrase).unwrap().unwrap();
        assert_eq!(rec.operator_id, "zal-7");
        assert_eq!(rec.station_id, "station-A");
        assert_eq!(rec.profile().full_name, "Дежурный оператор зала");
    }

    #[test]
    fn blob_does_not_contain_pin_plaintext() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        provision(tmp.path(), KeySource::Passphrase, &identity(), "135790").unwrap();
        let raw = std::fs::read(profile_path(tmp.path())).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"135790"));
    }

    #[test]
    fn verify_absent_is_false() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_provisioned(tmp.path()));
        assert!(!verify(tmp.path(), KeySource::Passphrase, "1234").unwrap());
    }

    // Fail-secure: без ключа станции блоб не читается — не «пустой профиль»,
    // а ошибка (наследует R-004/этап 13.5). Порченый блоб → StoreError::Crypto.
    #[test]
    fn tampered_blob_fails_secure() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        provision(tmp.path(), KeySource::Passphrase, &identity(), "2468").unwrap();
        let path = profile_path(tmp.path());
        let mut blob = std::fs::read(&path).unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        std::fs::write(&path, &blob).unwrap();
        assert!(matches!(
            verify(tmp.path(), KeySource::Passphrase, "2468"),
            Err(StoreError::Crypto(_))
        ));
        assert!(matches!(
            load(tmp.path(), KeySource::Passphrase),
            Err(StoreError::Crypto(_))
        ));
    }

    // Один тест на env-seeding: env-переменные process-global, поэтому весь их
    // жизненный цикл держим внутри одного теста (как у admin_pin).
    #[test]
    fn provision_from_env_seeds_once_and_requires_full_identity() {
        set_test_passphrase();
        let clear = || {
            for k in [
                OPERATOR_PIN_ENV,
                OPERATOR_NAME_ENV,
                OPERATOR_ROLE_ENV,
                crate::sync::OPERATOR_ID_ENV,
                crate::sync::STATION_ID_ENV,
            ] {
                std::env::remove_var(k);
            }
        };

        // Нет env — пропускаем.
        clear();
        let empty = tempfile::tempdir().unwrap();
        assert!(!provision_from_env_if_absent(empty.path(), KeySource::Passphrase, 4).unwrap());
        assert!(!is_provisioned(empty.path()));

        // Неполная идентичность (нет station_id) — не провижиним.
        std::env::set_var(OPERATOR_PIN_ENV, "246810");
        std::env::set_var(crate::sync::OPERATOR_ID_ENV, "zal-7");
        let partial = tempfile::tempdir().unwrap();
        assert!(!provision_from_env_if_absent(partial.path(), KeySource::Passphrase, 4).unwrap());
        assert!(!is_provisioned(partial.path()));

        // Слишком короткий PIN — не провижиним.
        std::env::set_var(crate::sync::STATION_ID_ENV, "station-A");
        std::env::set_var(OPERATOR_PIN_ENV, "12");
        let short = tempfile::tempdir().unwrap();
        assert!(!provision_from_env_if_absent(short.path(), KeySource::Passphrase, 4).unwrap());
        assert!(!is_provisioned(short.path()));

        // Полная идентичность + валидный PIN — засеивает один раз; повтор с другим
        // env не перезаписывает.
        std::env::set_var(OPERATOR_PIN_ENV, "246810");
        std::env::set_var(OPERATOR_NAME_ENV, "Оператор зала");
        std::env::set_var(OPERATOR_ROLE_ENV, "clerk");
        let tmp = tempfile::tempdir().unwrap();
        assert!(provision_from_env_if_absent(tmp.path(), KeySource::Passphrase, 4).unwrap());
        let rec = load(tmp.path(), KeySource::Passphrase).unwrap().unwrap();
        assert_eq!(rec.operator_id, "zal-7");
        assert_eq!(rec.station_id, "station-A");
        assert!(verify(tmp.path(), KeySource::Passphrase, "246810").unwrap());

        std::env::set_var(OPERATOR_PIN_ENV, "999999");
        assert!(!provision_from_env_if_absent(tmp.path(), KeySource::Passphrase, 4).unwrap());
        assert!(verify(tmp.path(), KeySource::Passphrase, "246810").unwrap());
        assert!(!verify(tmp.path(), KeySource::Passphrase, "999999").unwrap());

        clear();
    }
}
