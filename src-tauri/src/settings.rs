//! Типизированная схема настроек станции «Аудиопротокол».
//!
//! Зеркалит раздел **С** (станция) реестра [`docs/configuration.md`] — это
//! единственный источник истины для значений по умолчанию. В коде логики не
//! должно быть «магических чисел»: любой параметр читается из этой структуры.
//!
//! На этапе 00 модель только хранится/персистится (см. [`crate::ipc`]); на
//! запись она пока не влияет — эффект появится на этапах 01+.

use serde::{Deserialize, Serialize};

/// Мастер-кодек записи. WAV (PCM) — ради устойчивости к обрыву питания:
/// усечённый сегмент остаётся читаемым (см. `configuration.md` → «Аудио»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MasterCodec {
    WavPcm,
    Flac,
}

/// Политика удаления локальной копии после успешной выгрузки.
/// `configuration.md` → «Хранилище и ретеншн».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionMode {
    /// До подтверждения сервером + буферное окно (дефолт).
    UntilConfirmedPlusWindow,
    /// Удалять сразу по подтверждению (минимизация ПДн).
    DeleteOnConfirm,
    /// До ручного удаления.
    Manual,
}

/// Кодек архивной (сжатой) копии.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveCodec {
    Flac,
}

/// Источник ключа шифрования at-rest. `configuration.md` → «Хранилище и
/// ретеншн». Сам секрет в файле настроек **не хранится**: парольная фраза
/// приходит из env `COURT_AUDIO_STATION_PASSPHRASE`, эта настройка лишь выбирает
/// провайдер ключа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// Системное защищённое хранилище ОС (keychain/secret-service/DPAPI).
    /// Реальная интеграция — этап `08`; до неё рантайм откатывается к парольной
    /// фразе (см. `store::crypto`).
    OsKeystore,
    /// Производный ключ из парольной фразы станции (Argon2id) — оффлайн/headless.
    Passphrase,
}

// ── Аудио ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchiveCopySettings {
    pub enabled: bool,
    pub codec: ArchiveCodec,
}

impl Default for ArchiveCopySettings {
    fn default() -> Self {
        Self {
            // configuration.md: audio.archive_copy.enabled = false
            enabled: false,
            // configuration.md: audio.archive_copy.codec = flac
            codec: ArchiveCodec::Flac,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioSettings {
    /// `audio.device` — `None` означает «системное устройство по умолчанию».
    pub device: Option<String>,
    pub sample_rate_hz: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub master_codec: MasterCodec,
    /// `audio.capture_buffer_seconds` — глубина кольцевого буфера между
    /// аудио-callback'ом и consumer'ом (запас на джиттер планировщика).
    pub capture_buffer_seconds: f32,
    /// `audio.level_update_hz` — частота событий индикаторов уровня (~20–30 Гц).
    pub level_update_hz: u32,
    pub archive_copy: ArchiveCopySettings,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            device: None,
            // configuration.md: 44100 / 16 / 1 / wav_pcm
            sample_rate_hz: 44_100,
            bit_depth: 16,
            channels: 1,
            master_codec: MasterCodec::WavPcm,
            // configuration.md: audio.capture_buffer_seconds = 2.0
            capture_buffer_seconds: 2.0,
            // configuration.md: audio.level_update_hz = 25
            level_update_hz: 25,
            archive_copy: ArchiveCopySettings::default(),
        }
    }
}

// ── Запись и надёжность ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecorderSettings {
    pub segment_seconds: u32,
    pub flush_interval_ms: u32,
    pub max_session_hours: u32,
}

impl Default for RecorderSettings {
    fn default() -> Self {
        Self {
            // configuration.md: recorder.segment_seconds = 30
            segment_seconds: 30,
            // configuration.md: recorder.flush_interval_ms = 1500
            flush_interval_ms: 1_500,
            // configuration.md: recorder.max_session_hours = 12
            max_session_hours: 12,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceReconnectSettings {
    pub auto_resume: bool,
}

impl Default for DeviceReconnectSettings {
    fn default() -> Self {
        // configuration.md: reliability.device_reconnect.auto_resume = true
        Self { auto_resume: true }
    }
}

/// `reliability.mirror.*` — дефолты (`enabled=false`, `path=None`) совпадают с
/// type-defaults, поэтому `Default` выводится (`#[derive]`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MirrorSettings {
    pub enabled: bool,
    /// `reliability.mirror.path` — путь дублирующей дорожки (без дефолта).
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReliabilitySettings {
    pub watchdog_timeout_ms: u32,
    pub disk_low_threshold_mb: u64,
    pub disk_critical_mb: u64,
    pub device_reconnect: DeviceReconnectSettings,
    pub mirror: MirrorSettings,
}

impl Default for ReliabilitySettings {
    fn default() -> Self {
        Self {
            // configuration.md: reliability.watchdog_timeout_ms = 5000
            watchdog_timeout_ms: 5_000,
            // configuration.md: reliability.disk_low_threshold_mb = 1024
            disk_low_threshold_mb: 1_024,
            // configuration.md: reliability.disk_critical_mb = 256
            disk_critical_mb: 256,
            device_reconnect: DeviceReconnectSettings::default(),
            mirror: MirrorSettings::default(),
        }
    }
}

// ── Хранилище и ретеншн ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageSettings {
    /// `storage.root_path` — `None` означает `<data-dir>/recordings`
    /// (резолвится в рантайме по системному data-каталогу).
    pub root_path: Option<String>,
    pub encrypt_at_rest: bool,
    /// `storage.key_source` — откуда брать ключ шифрования at-rest.
    pub key_source: KeySource,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            root_path: None,
            // configuration.md: storage.encrypt_at_rest = true
            encrypt_at_rest: true,
            // configuration.md: storage.key_source = os_keystore
            key_source: KeySource::OsKeystore,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetentionSettings {
    pub mode: RetentionMode,
    pub require_integrity_verified: bool,
    pub safety_window_hours: u32,
}

impl Default for RetentionSettings {
    fn default() -> Self {
        Self {
            // configuration.md: retention.mode = until_confirmed_plus_window
            mode: RetentionMode::UntilConfirmedPlusWindow,
            // configuration.md: retention.require_integrity_verified = true
            require_integrity_verified: true,
            // configuration.md: retention.safety_window_hours = 72
            safety_window_hours: 72,
        }
    }
}

// ── Целостность ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntegritySettings {
    /// `integrity.segment_hash` — алгоритм хеша сегмента (дефолт `sha256`).
    pub segment_hash: String,
    pub hash_chain: bool,
    pub event_log: bool,
    /// `integrity.gost_sign` — ГОСТ ЭЦП (фаза 2).
    pub gost_sign: bool,
}

impl Default for IntegritySettings {
    fn default() -> Self {
        Self {
            // configuration.md: integrity.segment_hash = sha256
            segment_hash: "sha256".to_string(),
            // configuration.md: integrity.hash_chain = true
            hash_chain: true,
            // configuration.md: integrity.event_log = true
            event_log: true,
            // configuration.md: integrity.gost_sign = false (фаза 2)
            gost_sign: false,
        }
    }
}

// ── Выгрузка и сеть ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfflineQueueSettings {
    pub enabled: bool,
}

impl Default for OfflineQueueSettings {
    fn default() -> Self {
        // configuration.md: sync.offline_queue.enabled = true
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrySettings {
    pub backoff_base_ms: u32,
    pub backoff_max_ms: u32,
    /// `sync.retry.max_attempts` — 0 означает «без лимита, до успеха».
    pub max_attempts: u32,
}

impl Default for RetrySettings {
    fn default() -> Self {
        Self {
            // configuration.md: sync.retry.backoff_base_ms = 2000
            backoff_base_ms: 2_000,
            // configuration.md: sync.retry.backoff_max_ms = 60000
            backoff_max_ms: 60_000,
            // configuration.md: sync.retry.max_attempts = 0 (без лимита)
            max_attempts: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncSettings {
    /// `sync.server_base_url` — обязателен для выгрузки; задаётся оператором.
    pub server_base_url: Option<String>,
    pub chunk_size_mb: u32,
    pub parallel_uploads: u32,
    pub auto_upload: bool,
    pub defer_during_recording: bool,
    pub offline_queue: OfflineQueueSettings,
    pub retry: RetrySettings,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            server_base_url: None,
            // configuration.md: sync.chunk_size_mb = 8
            chunk_size_mb: 8,
            // configuration.md: sync.parallel_uploads = 1
            parallel_uploads: 1,
            // configuration.md: sync.auto_upload = true
            auto_upload: true,
            // configuration.md: sync.defer_during_recording = false
            defer_during_recording: false,
            offline_queue: OfflineQueueSettings::default(),
            retry: RetrySettings::default(),
        }
    }
}

// ── Аутентификация ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StationIdentitySettings {
    pub required: bool,
}

impl Default for StationIdentitySettings {
    fn default() -> Self {
        // configuration.md: auth.station_identity.required = true
        Self { required: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperatorAuthSettings {
    pub required_to_start: bool,
    pub cached_session_hours: u32,
}

impl Default for OperatorAuthSettings {
    fn default() -> Self {
        Self {
            // configuration.md: auth.operator.required_to_start = true
            required_to_start: true,
            // configuration.md: auth.operator.cached_session_hours = 24
            cached_session_hours: 24,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthSettings {
    pub station_identity: StationIdentitySettings,
    pub operator: OperatorAuthSettings,
    pub recording_survives_token_expiry: bool,
}

impl Default for AuthSettings {
    fn default() -> Self {
        Self {
            station_identity: StationIdentitySettings::default(),
            operator: OperatorAuthSettings::default(),
            // configuration.md: auth.recording_survives_token_expiry = true
            recording_survives_token_expiry: true,
        }
    }
}

// ── Привязка к делу (кэш дел) ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseCacheSettings {
    pub enabled: bool,
    pub encrypt: bool,
    /// `case_cache.scope` — какие дела кэшировать (дефолт `court_docket`).
    pub scope: String,
    pub ttl_hours: u32,
    pub max_records: u32,
}

impl Default for CaseCacheSettings {
    fn default() -> Self {
        Self {
            // configuration.md: case_cache.enabled = true
            enabled: true,
            // configuration.md: case_cache.encrypt = true
            encrypt: true,
            // configuration.md: case_cache.scope = court_docket
            scope: "court_docket".to_string(),
            // configuration.md: case_cache.ttl_hours = 24
            ttl_hours: 24,
            // configuration.md: case_cache.max_records = 500
            max_records: 500,
        }
    }
}

// ── Корневая модель ───────────────────────────────────────────────────────────

/// Полная схема настроек станции. Сериализуется в JSON (файл конфигурации
/// Tauri); при отсутствии файла используется [`Settings::default`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub audio: AudioSettings,
    pub recorder: RecorderSettings,
    pub reliability: ReliabilitySettings,
    pub storage: StorageSettings,
    pub retention: RetentionSettings,
    pub integrity: IntegritySettings,
    pub sync: SyncSettings,
    pub auth: AuthSettings,
    pub case_cache: CaseCacheSettings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_registry() {
        let s = Settings::default();
        assert_eq!(s.audio.sample_rate_hz, 44_100);
        assert_eq!(s.audio.bit_depth, 16);
        assert_eq!(s.audio.channels, 1);
        assert_eq!(s.audio.master_codec, MasterCodec::WavPcm);
        assert_eq!(s.audio.capture_buffer_seconds, 2.0);
        assert_eq!(s.audio.level_update_hz, 25);
        assert_eq!(s.recorder.segment_seconds, 30);
        assert_eq!(s.recorder.flush_interval_ms, 1_500);
        assert!(s.storage.encrypt_at_rest);
        assert_eq!(s.storage.key_source, KeySource::OsKeystore);
        assert_eq!(s.retention.mode, RetentionMode::UntilConfirmedPlusWindow);
        assert!(s.retention.require_integrity_verified);
        assert_eq!(s.retention.safety_window_hours, 72);
        assert_eq!(s.integrity.segment_hash, "sha256");
        assert!(s.integrity.hash_chain);
        assert_eq!(s.sync.chunk_size_mb, 8);
        assert_eq!(s.sync.retry.max_attempts, 0);
    }

    #[test]
    fn roundtrips_through_json() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).expect("serialize");
        let back: Settings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
    }

    #[test]
    fn partial_json_fills_defaults() {
        // `#[serde(default)]` на корне: неполный JSON не должен ломать загрузку.
        let back: Settings = serde_json::from_str("{}").expect("deserialize empty");
        assert_eq!(back, Settings::default());
    }
}
