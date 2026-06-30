//! Зашифрованный локальный кэш дел для оффлайн-привязки (этап 05 —
//! `promts/05_case_binding_offline.md`, deliverable 1–2).
//!
//! Станция кэширует **докет суда/зала** (`case_cache.scope`), чтобы привязывать
//! запись к делу **без сети**. Кэш содержит ПДн (№ дела, ФИО) → хранится
//! **at-rest зашифрованным** (`case_cache.encrypt`, AES-256-GCM через
//! [`super::crypto`]) отдельным блоб-файлом, а **не** в plaintext-манифесте
//! SQLite. Минимизация ПДн: минимальный состав полей ([`CaseRecord`]), лимит
//! [`CaseCacheSettings::max_records`], устаревание по
//! [`CaseCacheSettings::ttl_hours`].
//!
//! **Транспорт** (HTTP к slim-эндпоинту докета) вынесен за trait
//! [`CaseDocketFetcher`]: реальный фетчер и его подключение — этапы `06`/`07`
//! (сеть + операторская авторизация + серверный эндпоинт). Здесь — фетчер-
//! агностичная логика кэша (скоуп/лимит/ttl/шифрование/поиск), полностью
//! тестируемая оффлайн на фейк-фетчере.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::crypto;
use super::StoreError;
use crate::settings::{CaseCacheSettings, KeySource};

/// Имя зашифрованного файла кэша в корне хранилища.
pub const CACHE_ENC_FILE: &str = "case_cache.enc";
/// Имя открытого файла кэша (когда `case_cache.encrypt = false`).
pub const CACHE_PLAIN_FILE: &str = "case_cache.json";

/// Запись дела в кэше — минимальный состав полей (ПДн-минимизация).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseRecord {
    /// Идентификатор `Adjudication` на сервере (для `resolved`-привязки).
    pub id: String,
    /// № дела (как в докете).
    pub number: String,
    /// ФИО сторон/подсудимых (одной строкой, как отдаёт докет).
    pub fio: String,
    /// Дата заседания/дела (строкой `YYYY-MM-DD`, как в докете).
    pub date: String,
}

impl CaseRecord {
    /// Совпадает ли запись с поисковым запросом (регистронезависимо, по №/ФИО).
    fn matches(&self, needle_lower: &str) -> bool {
        self.number.to_lowercase().contains(needle_lower)
            || self.fio.to_lowercase().contains(needle_lower)
    }
}

/// Локальный кэш дел + метаданные свежести/скоупа.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseCache {
    /// Когда кэш синхронизирован (unix ms) — основа индикатора свежести/ttl.
    pub synced_at_unix_ms: u64,
    /// Скоуп докета, из которого набран кэш (`case_cache.scope`).
    pub scope: String,
    pub records: Vec<CaseRecord>,
}

impl CaseCache {
    /// Регистронезависимый поиск по №/ФИО (оффлайн). Пустой запрос → весь кэш.
    pub fn search(&self, query: &str) -> Vec<CaseRecord> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return self.records.clone();
        }
        self.records
            .iter()
            .filter(|r| r.matches(&needle))
            .cloned()
            .collect()
    }

    /// Свеж ли кэш относительно `ttl_hours` (порог из реестра конфигурации).
    pub fn is_fresh(&self, ttl_hours: u32, now_unix_ms: u64) -> bool {
        let ttl_ms = (ttl_hours as u64).saturating_mul(60 * 60 * 1000);
        now_unix_ms.saturating_sub(self.synced_at_unix_ms) <= ttl_ms
    }
}

/// Источник списка дел (докета). Реальная реализация (HTTP к `ex_system`) —
/// этап `06`/`07`; здесь только контракт seam'а.
pub trait CaseDocketFetcher {
    /// Получить дела докета в заданном скоупе, не более `limit` записей.
    fn fetch(&self, scope: &str, limit: u32) -> Result<Vec<CaseRecord>, String>;
}

/// Синхронизировать кэш через фетчер: тянет докет, **обрезает до `max_records`**
/// (минимизация ПДн), фиксирует `scope` и момент синхронизации. Не пишет на диск
/// — это делает вызывающий через [`save`] (так логика остаётся чистой/тестируемой).
pub fn sync_into_cache(
    fetcher: &dyn CaseDocketFetcher,
    scope: &str,
    max_records: u32,
    now_unix_ms: u64,
) -> Result<CaseCache, StoreError> {
    let mut records = fetcher
        .fetch(scope, max_records)
        .map_err(|e| StoreError::Io(format!("синхронизация кэша дел: {e}")))?;
    // Жёстко гарантируем лимит даже если сервер вернул больше запрошенного.
    records.truncate(max_records as usize);
    Ok(CaseCache {
        synced_at_unix_ms: now_unix_ms,
        scope: scope.to_string(),
        records,
    })
}

/// Путь файла кэша по флагу шифрования.
fn cache_path(root: &Path, encrypt: bool) -> PathBuf {
    root.join(if encrypt {
        CACHE_ENC_FILE
    } else {
        CACHE_PLAIN_FILE
    })
}

/// Сохранить кэш на диск: зашифрованным (`case_cache.encrypt = true`,
/// AES-256-GCM ключом станции) либо открытым JSON. Создаёт корень при отсутствии.
pub fn save(
    root: &Path,
    settings: &CaseCacheSettings,
    key_source: KeySource,
    cache: &CaseCache,
) -> Result<(), StoreError> {
    std::fs::create_dir_all(root)?;
    let json = serde_json::to_vec(cache)?;
    let path = cache_path(root, settings.encrypt);
    if settings.encrypt {
        let key = crypto::resolve_station_key(key_source, root)?;
        let blob = crypto::encrypt_bytes(&json, &key)?;
        std::fs::write(&path, blob)?;
    } else {
        std::fs::write(&path, &json)?;
    }
    Ok(())
}

/// Прочитать кэш с диска (или `None`, если его ещё нет). Дешифрует при
/// `case_cache.encrypt = true`.
pub fn load(
    root: &Path,
    settings: &CaseCacheSettings,
    key_source: KeySource,
) -> Result<Option<CaseCache>, StoreError> {
    let path = cache_path(root, settings.encrypt);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read(&path)?;
    let json = if settings.encrypt {
        let key = crypto::resolve_station_key(key_source, root)?;
        crypto::decrypt_bytes(&raw, &key)?
    } else {
        raw
    };
    let cache: CaseCache = serde_json::from_slice(&json)?;
    Ok(Some(cache))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Детерминированный ключ станции для тестов (env не трогаем).
    fn set_test_passphrase() {
        std::env::set_var(crypto::PASSPHRASE_ENV, "station-test-secret");
    }

    fn settings(encrypt: bool) -> CaseCacheSettings {
        CaseCacheSettings {
            enabled: true,
            encrypt,
            scope: "court_docket".into(),
            ttl_hours: 24,
            max_records: 500,
        }
    }

    fn rec(id: &str, number: &str, fio: &str) -> CaseRecord {
        CaseRecord {
            id: id.into(),
            number: number.into(),
            fio: fio.into(),
            date: "2026-06-30".into(),
        }
    }

    fn sample_cache() -> CaseCache {
        CaseCache {
            synced_at_unix_ms: 1_700_000_000_000,
            scope: "court_docket".into(),
            records: vec![
                rec("adj-1", "№ 1-123/2026", "Иванов Иван Иванович"),
                rec("adj-2", "№ 2-7/2026", "Петрова Анна Сергеевна"),
            ],
        }
    }

    /// Фейк-фетчер для оффлайн-тестов транспортного seam'а.
    struct FakeFetcher {
        records: Vec<CaseRecord>,
        fail: bool,
    }

    impl CaseDocketFetcher for FakeFetcher {
        fn fetch(&self, _scope: &str, limit: u32) -> Result<Vec<CaseRecord>, String> {
            if self.fail {
                return Err("нет сети".into());
            }
            Ok(self.records.iter().take(limit as usize).cloned().collect())
        }
    }

    #[test]
    fn encrypted_roundtrip_save_load() {
        set_test_passphrase();
        let tmp = tempfile::tempdir().unwrap();
        let cache = sample_cache();
        save(tmp.path(), &settings(true), KeySource::Passphrase, &cache).unwrap();

        // На диске лежит .enc, а не открытый JSON, и в нём нет ПДн открытым текстом.
        let enc_path = tmp.path().join(CACHE_ENC_FILE);
        assert!(enc_path.exists());
        assert!(!tmp.path().join(CACHE_PLAIN_FILE).exists());
        // ПДн не лежат открытым текстом: ни скоупа, ни ФИО в шифр-блобе нет.
        let blob = std::fs::read(&enc_path).unwrap();
        let needle = "court_docket".as_bytes();
        assert!(!blob.windows(needle.len()).any(|w| w == needle));
        let fio = "Иванов".as_bytes();
        assert!(!blob.windows(fio.len()).any(|w| w == fio));

        let back = load(tmp.path(), &settings(true), KeySource::Passphrase)
            .unwrap()
            .unwrap();
        assert_eq!(back, cache);
    }

    #[test]
    fn plain_save_is_readable_json() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = sample_cache();
        save(tmp.path(), &settings(false), KeySource::Passphrase, &cache).unwrap();
        let path = tmp.path().join(CACHE_PLAIN_FILE);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("court_docket"));
        let back = load(tmp.path(), &settings(false), KeySource::Passphrase)
            .unwrap()
            .unwrap();
        assert_eq!(back, cache);
    }

    #[test]
    fn load_absent_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load(tmp.path(), &settings(true), KeySource::Passphrase)
            .unwrap()
            .is_none());
    }

    #[test]
    fn search_by_number_and_fio_case_insensitive() {
        let cache = sample_cache();
        assert_eq!(cache.search("1-123").len(), 1);
        assert_eq!(cache.search("петров").len(), 1); // регистр игнорируется
        assert_eq!(cache.search("2026").len(), 2); // обе записи
        assert_eq!(cache.search("нет-такого").len(), 0);
        assert_eq!(cache.search("").len(), 2); // пустой запрос — весь кэш
    }

    #[test]
    fn ttl_freshness() {
        let cache = sample_cache();
        let ttl_hours = 24;
        // В пределах 24 ч — свежий.
        let within = cache.synced_at_unix_ms + 23 * 60 * 60 * 1000;
        assert!(cache.is_fresh(ttl_hours, within));
        // За пределами — устарел.
        let beyond = cache.synced_at_unix_ms + 25 * 60 * 60 * 1000;
        assert!(!cache.is_fresh(ttl_hours, beyond));
    }

    #[test]
    fn sync_truncates_to_max_records_and_sets_scope() {
        let fetcher = FakeFetcher {
            records: (0..10)
                .map(|i| rec(&format!("adj-{i}"), &format!("№ {i}/2026"), "Тест"))
                .collect(),
            fail: false,
        };
        let cache = sync_into_cache(&fetcher, "court_docket", 3, 42).unwrap();
        assert_eq!(cache.records.len(), 3); // лимит соблюдён
        assert_eq!(cache.scope, "court_docket");
        assert_eq!(cache.synced_at_unix_ms, 42);
    }

    #[test]
    fn sync_offline_errors_and_keeps_existing_cache() {
        // Оффлайн-сценарий: фетчер падает → sync возвращает ошибку, ранее
        // сохранённый кэш на диске остаётся нетронутым (его не перезаписываем).
        let tmp = tempfile::tempdir().unwrap();
        let existing = sample_cache();
        save(
            tmp.path(),
            &settings(false),
            KeySource::Passphrase,
            &existing,
        )
        .unwrap();

        let fetcher = FakeFetcher {
            records: vec![],
            fail: true,
        };
        let res = sync_into_cache(&fetcher, "court_docket", 500, 99);
        assert!(matches!(res, Err(StoreError::Io(_))));

        // Кэш на диске не изменился — привязка по нему всё ещё работает.
        let still = load(tmp.path(), &settings(false), KeySource::Passphrase)
            .unwrap()
            .unwrap();
        assert_eq!(still, existing);
    }
}
