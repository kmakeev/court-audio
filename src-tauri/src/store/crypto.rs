//! Шифрование сегментов at-rest (этап 03 — `promts/03_store_integrity.md`,
//! deliverable 4 / шаг 5).
//!
//! AES-256-GCM на **финализированный** сегмент: во время записи сегмент лежит
//! открытым WAV (crash-safe, чинится в этапе 02); по завершении считаем хеш по
//! каноничному контенту ([`crate::integrity::hash`]) → шифруем → удаляем
//! открытую копию. Шифрование **не входит** в хеш (сервер верифицирует контент,
//! не шифртекст) и не ломает надёжность (усечённый сегмент остаётся
//! восстановимым до финализации).
//!
//! Ключ — 256-бит ключ станции. В этапе 03 реализован **passphrase-провайдер**
//! (Argon2id из env `COURT_AUDIO_STATION_PASSPHRASE` + персистентная соль);
//! OS-keystore — заглушка ([`OsKeystoreKeyProvider`]), реальная интеграция —
//! этап `08`. Выбор источника — `Settings.storage.key_source`.

use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::Argon2;
use rand::RngCore;

use crate::integrity::hash;
use crate::settings::KeySource;

/// Длина nonce AES-GCM — 96 бит (стандарт GCM, не настройка).
const NONCE_LEN: usize = 12;
/// Длина соли KDF — 128 бит (крипто-константа Argon2id, не настройка).
const SALT_LEN: usize = 16;
/// Размер ключа станции — 256 бит (AES-256).
const KEY_LEN: usize = 32;
/// Имя файла персистентной соли в корне хранилища.
pub const SALT_FILE_NAME: &str = "key.salt";
/// Env-переменная с парольной фразой станции (секрет, не в settings.json).
pub const PASSPHRASE_ENV: &str = "COURT_AUDIO_STATION_PASSPHRASE";

/// Ошибка ключа/шифрования.
#[derive(Debug)]
pub enum CryptoError {
    /// OS-keystore недоступен (headless/оффлайн или этап 08 ещё не интегрирован).
    KeystoreUnavailable,
    /// Парольная фраза не задана (env пуст), а keystore недоступен.
    PassphraseMissing,
    /// Ошибка KDF (Argon2id).
    Kdf(String),
    /// Ошибка AEAD (шифрование/дешифрование, в т.ч. неверный тег = подмена).
    Cipher(String),
    /// Файл слишком короткий, чтобы содержать nonce + тег.
    Malformed(String),
    /// Ошибка ввода-вывода.
    Io(String),
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoError::KeystoreUnavailable => write!(f, "OS-keystore недоступен"),
            CryptoError::PassphraseMissing => {
                write!(
                    f,
                    "парольная фраза станции не задана (env {PASSPHRASE_ENV})"
                )
            }
            CryptoError::Kdf(e) => write!(f, "ошибка KDF Argon2id: {e}"),
            CryptoError::Cipher(e) => write!(f, "ошибка AES-256-GCM: {e}"),
            CryptoError::Malformed(e) => write!(f, "повреждённый шифр-файл: {e}"),
            CryptoError::Io(e) => write!(f, "ошибка ввода-вывода: {e}"),
        }
    }
}

impl std::error::Error for CryptoError {}

impl From<CryptoError> for super::StoreError {
    fn from(e: CryptoError) -> Self {
        super::StoreError::Crypto(e.to_string())
    }
}

impl From<std::io::Error> for CryptoError {
    fn from(e: std::io::Error) -> Self {
        CryptoError::Io(e.to_string())
    }
}

/// Источник 256-бит ключа станции.
pub trait KeyProvider {
    /// Получить (или вывести) ключ станции.
    fn station_key(&self) -> Result<[u8; KEY_LEN], CryptoError>;
}

/// Заглушка OS-keystore: реальная реализация (keychain/secret-service/DPAPI) —
/// этап `08`. Сейчас всегда сообщает о недоступности, чтобы рантайм откатился к
/// парольной фразе.
pub struct OsKeystoreKeyProvider;

impl KeyProvider for OsKeystoreKeyProvider {
    fn station_key(&self) -> Result<[u8; KEY_LEN], CryptoError> {
        Err(CryptoError::KeystoreUnavailable)
    }
}

/// Производный ключ из парольной фразы станции (Argon2id, рекомендованные
/// параметры крейта). Соль стабильна между рестартами, иначе ключ не
/// воспроизводится.
pub struct PassphraseKeyProvider {
    passphrase: String,
    salt: Vec<u8>,
}

impl PassphraseKeyProvider {
    /// Явный конструктор (тесты/интеграция).
    pub fn new(passphrase: impl Into<String>, salt: Vec<u8>) -> Self {
        Self {
            passphrase: passphrase.into(),
            salt,
        }
    }

    /// Собрать из env-парольной фразы и персистентной соли в корне хранилища.
    pub fn from_env(root: &Path) -> Result<Self, CryptoError> {
        let passphrase = std::env::var(PASSPHRASE_ENV)
            .ok()
            .filter(|p| !p.is_empty())
            .ok_or(CryptoError::PassphraseMissing)?;
        let salt = load_or_create_salt(root)?;
        Ok(Self::new(passphrase, salt))
    }
}

impl KeyProvider for PassphraseKeyProvider {
    fn station_key(&self) -> Result<[u8; KEY_LEN], CryptoError> {
        let mut key = [0u8; KEY_LEN];
        Argon2::default()
            .hash_password_into(self.passphrase.as_bytes(), &self.salt, &mut key)
            .map_err(|e| CryptoError::Kdf(e.to_string()))?;
        Ok(key)
    }
}

/// Прочитать соль из `<root>/key.salt`, создав её (128 бит из ОС-CSPRNG) при
/// отсутствии. Соль не секретна, но должна быть стабильна.
pub fn load_or_create_salt(root: &Path) -> Result<Vec<u8>, CryptoError> {
    std::fs::create_dir_all(root)?;
    let path = root.join(SALT_FILE_NAME);
    if path.exists() {
        return Ok(std::fs::read(&path)?);
    }
    let mut salt = vec![0u8; SALT_LEN];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    std::fs::write(&path, &salt)?;
    Ok(salt)
}

/// Разрешить ключ станции по настройке `storage.key_source`. Для `OsKeystore`
/// (пока заглушка) — фолбэк на парольную фразу: гарантирует работу на
/// оффлайн/headless-станции.
pub fn resolve_station_key(
    key_source: KeySource,
    root: &Path,
) -> Result<[u8; KEY_LEN], CryptoError> {
    match key_source {
        KeySource::Passphrase => PassphraseKeyProvider::from_env(root)?.station_key(),
        KeySource::OsKeystore => match OsKeystoreKeyProvider.station_key() {
            Ok(k) => Ok(k),
            Err(CryptoError::KeystoreUnavailable) => {
                PassphraseKeyProvider::from_env(root)?.station_key()
            }
            Err(e) => Err(e),
        },
    }
}

/// Зашифровать буфер: формат `[12B nonce][ciphertext+tag]`.
pub fn encrypt_bytes(plain: &[u8], key: &[u8; KEY_LEN]) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| CryptoError::Cipher(e.to_string()))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plain)
        .map_err(|e| CryptoError::Cipher(e.to_string()))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Расшифровать буфер формата [`encrypt_bytes`]. Неверный тег (подмена) даёт
/// [`CryptoError::Cipher`].
pub fn decrypt_bytes(data: &[u8], key: &[u8; KEY_LEN]) -> Result<Vec<u8>, CryptoError> {
    if data.len() < NONCE_LEN {
        return Err(CryptoError::Malformed("короче nonce".into()));
    }
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| CryptoError::Cipher(e.to_string()))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| CryptoError::Cipher(e.to_string()))
}

/// Итог финализации сегмента: хеш каноничного контента и где он лежит на диске.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedSegment {
    /// SHA-256 каноничного контента (WAV до шифрования), hex.
    pub content_sha256: String,
    /// Размер каноничного контента в байтах.
    pub content_size_bytes: u64,
    /// Путь к хранимому файлу (`.enc` при шифровании, иначе исходный WAV).
    pub stored_path: PathBuf,
    /// Был ли сегмент зашифрован.
    pub encrypted: bool,
}

/// Расширение, добавляемое к зашифрованному сегменту.
pub const ENCRYPTED_EXT: &str = "enc";

/// Финализировать сегмент: посчитать хеш по открытому WAV, затем (если
/// `encrypt_at_rest`) зашифровать в `<path>.enc` и удалить открытую копию.
pub fn finalize_segment(
    plain_path: &Path,
    key: Option<&[u8; KEY_LEN]>,
    encrypt_at_rest: bool,
) -> Result<FinalizedSegment, CryptoError> {
    let content_sha256 =
        hash::sha256_file(plain_path).map_err(|e| CryptoError::Io(e.to_string()))?;
    let content_size_bytes = std::fs::metadata(plain_path)?.len();

    if !encrypt_at_rest {
        return Ok(FinalizedSegment {
            content_sha256,
            content_size_bytes,
            stored_path: plain_path.to_path_buf(),
            encrypted: false,
        });
    }

    let key = key.ok_or(CryptoError::PassphraseMissing)?;
    let plain = std::fs::read(plain_path)?;
    let encrypted = encrypt_bytes(&plain, key)?;
    let enc_path = enc_path_for(plain_path);
    std::fs::write(&enc_path, &encrypted)?;
    // Открытую копию удаляем только после успешной записи шифр-файла.
    std::fs::remove_file(plain_path)?;

    Ok(FinalizedSegment {
        content_sha256,
        content_size_bytes,
        stored_path: enc_path,
        encrypted: true,
    })
}

/// Прозрачно прочитать каноничный контент сегмента для выгрузки (`06`):
/// дешифрует `.enc`, иначе читает как есть.
pub fn read_segment_plain(
    stored_path: &Path,
    key: Option<&[u8; KEY_LEN]>,
) -> Result<Vec<u8>, CryptoError> {
    let is_encrypted = stored_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e == ENCRYPTED_EXT)
        .unwrap_or(false);
    if is_encrypted {
        let key = key.ok_or(CryptoError::PassphraseMissing)?;
        let data = std::fs::read(stored_path)?;
        decrypt_bytes(&data, key)
    } else {
        Ok(std::fs::read(stored_path)?)
    }
}

/// Путь зашифрованного файла: `<plain>.enc`.
pub fn enc_path_for(plain_path: &Path) -> PathBuf {
    let mut name = plain_path.as_os_str().to_os_string();
    name.push(".");
    name.push(ENCRYPTED_EXT);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; KEY_LEN] {
        // Детерминированный ключ из явной соли+фразы (env не трогаем).
        PassphraseKeyProvider::new("station-secret", b"0123456789abcdef".to_vec())
            .station_key()
            .unwrap()
    }

    #[test]
    fn keystore_stub_is_unavailable() {
        assert!(matches!(
            OsKeystoreKeyProvider.station_key(),
            Err(CryptoError::KeystoreUnavailable)
        ));
    }

    #[test]
    fn passphrase_kdf_is_deterministic() {
        let salt = b"fixed-salt-16byt".to_vec();
        let k1 = PassphraseKeyProvider::new("pw", salt.clone())
            .station_key()
            .unwrap();
        let k2 = PassphraseKeyProvider::new("pw", salt)
            .station_key()
            .unwrap();
        assert_eq!(k1, k2);
        // Другая фраза — другой ключ.
        let k3 = PassphraseKeyProvider::new("pw2", b"fixed-salt-16byt".to_vec())
            .station_key()
            .unwrap();
        assert_ne!(k1, k3);
    }

    #[test]
    fn salt_persists_and_is_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let s1 = load_or_create_salt(tmp.path()).unwrap();
        let s2 = load_or_create_salt(tmp.path()).unwrap();
        assert_eq!(s1, s2);
        assert_eq!(s1.len(), SALT_LEN);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = test_key();
        let plain = b"canonical WAV segment bytes \x00\x01\x02";
        let enc = encrypt_bytes(plain, &key).unwrap();
        assert_ne!(&enc[NONCE_LEN..], &plain[..]); // действительно зашифровано
        let dec = decrypt_bytes(&enc, &key).unwrap();
        assert_eq!(dec, plain);
    }

    #[test]
    fn tampered_ciphertext_fails_auth() {
        let key = test_key();
        let mut enc = encrypt_bytes(b"secret", &key).unwrap();
        let last = enc.len() - 1;
        enc[last] ^= 0xff; // портим тег/шифртекст
        assert!(matches!(
            decrypt_bytes(&enc, &key),
            Err(CryptoError::Cipher(_))
        ));
    }

    #[test]
    fn finalize_encrypts_and_removes_plain() {
        let tmp = tempfile::tempdir().unwrap();
        let plain_path = tmp.path().join("seg-0001.wav");
        let content = b"RIFF....WAVE canonical content";
        std::fs::write(&plain_path, content).unwrap();
        let expected_hash = hash::sha256_bytes(content);
        let key = test_key();

        let fin = finalize_segment(&plain_path, Some(&key), true).unwrap();
        assert!(fin.encrypted);
        assert_eq!(fin.content_sha256, expected_hash);
        assert_eq!(fin.content_size_bytes, content.len() as u64);
        // Открытая копия удалена, шифр-файл на месте.
        assert!(!plain_path.exists());
        assert!(fin.stored_path.exists());
        assert_eq!(fin.stored_path, enc_path_for(&plain_path));

        // Прозрачное чтение восстанавливает каноничный контент.
        let read = read_segment_plain(&fin.stored_path, Some(&key)).unwrap();
        assert_eq!(read, content);
        // Хеш контента совпадает до и после шифрования.
        assert_eq!(hash::sha256_bytes(&read), expected_hash);
    }

    #[test]
    fn finalize_without_encryption_keeps_plain() {
        let tmp = tempfile::tempdir().unwrap();
        let plain_path = tmp.path().join("seg-0001.wav");
        let content = b"plain content";
        std::fs::write(&plain_path, content).unwrap();

        let fin = finalize_segment(&plain_path, None, false).unwrap();
        assert!(!fin.encrypted);
        assert_eq!(fin.stored_path, plain_path);
        assert!(plain_path.exists());
        let read = read_segment_plain(&fin.stored_path, None).unwrap();
        assert_eq!(read, content);
    }
}
