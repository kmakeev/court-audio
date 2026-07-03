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
    // ── Многоканал по ролям (фаза 2, `promts/09_multichannel.md`) ──
    // Новые поля с `#[serde(default)]`: конфиги v1 (без этих ключей) грузятся
    // без ошибок и получают одноканальное поведение.
    /// `audio.multichannel` — включение многоканального захвата.
    #[serde(default)]
    pub multichannel: MultichannelSettings,
    /// `audio.tracks` — карта дорожек; пусто → один трек из `device`/`channels`.
    #[serde(default)]
    pub tracks: Vec<TrackConfig>,
    /// `audio.roles` — справочник ролей дорожек (роль трека обязана быть отсюда).
    #[serde(default = "default_roles")]
    pub roles: Vec<String>,
    /// `audio.sync` — единый клок и компенсация дрейфа между дорожками.
    #[serde(default)]
    pub sync: AudioSyncSettings,
    /// `audio.master_downmix` — опц. сведённый мастер поверх пофайловых дорожек.
    #[serde(default)]
    pub master_downmix: MasterDownmixSettings,
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
            multichannel: MultichannelSettings::default(),
            tracks: Vec::new(),
            roles: default_roles(),
            sync: AudioSyncSettings::default(),
            master_downmix: MasterDownmixSettings::default(),
        }
    }
}

/// Судебные роли по умолчанию (`configuration.md` → `audio.roles`).
fn default_roles() -> Vec<String> {
    ["judge", "clerk", "prosecution", "defense", "witness", "room"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// `audio.multichannel` — тумблер многоканального режима.
/// configuration.md: `audio.multichannel.enabled = false` (аддитивно, v1 по
/// умолчанию — `Default` даёт `false`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MultichannelSettings {
    pub enabled: bool,
}

/// Одна дорожка в карте «канал ↔ роль» (`audio.tracks[*]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackConfig {
    /// Устройство источника; `None` — системное по умолчанию (как `audio.device`).
    #[serde(default)]
    pub device: Option<String>,
    /// Индекс канала в интерливнутом потоке устройства (0-based).
    #[serde(default)]
    pub channel_index: u16,
    /// Роль дорожки; обязана присутствовать в `audio.roles`.
    pub role: String,
    /// Человекочитаемая метка дорожки (для UI/имён файлов).
    #[serde(default)]
    pub label: String,
}

/// `audio.sync` — единый клок сессии и контроль дрейфа между дорожками.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioSyncSettings {
    pub clock_master_track: u16,
    pub drift_threshold_ms: u32,
    pub drift_compensate: bool,
}

impl Default for AudioSyncSettings {
    fn default() -> Self {
        Self {
            // configuration.md: audio.sync.clock_master_track = 0
            clock_master_track: 0,
            // configuration.md: audio.sync.drift_threshold_ms = 50
            drift_threshold_ms: 50,
            // configuration.md: audio.sync.drift_compensate = true
            drift_compensate: true,
        }
    }
}

/// `audio.master_downmix` — опц. сведённый мастер поверх пофайловых дорожек.
/// configuration.md: `audio.master_downmix.enabled = false` (`Default` → `false`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MasterDownmixSettings {
    pub enabled: bool,
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

/// `auth.operator.offline_pin` — PIN как второй фактор для оффлайн-старта по
/// кэшированной сессии (этап 10.3). Сам PIN/его хеш в settings.json **не
/// хранится** — только политика; хеш Argon2id лежит в зашифрованном блобе
/// кэш-сессии (`store::auth_cache`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfflinePinSettings {
    pub required: bool,
    pub min_length: u32,
}

impl Default for OfflinePinSettings {
    fn default() -> Self {
        Self {
            // configuration.md: auth.operator.offline_pin.required = true
            required: true,
            // configuration.md: auth.operator.offline_pin.min_length = 4
            min_length: 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperatorAuthSettings {
    pub required_to_start: bool,
    pub cached_session_hours: u32,
    /// `auth.operator.offline_pin` — PIN оффлайн-разблокировки (этап 10.3).
    /// `#[serde(default)]`: конфиги до 10.3 грузятся и получают дефолт реестра.
    #[serde(default)]
    pub offline_pin: OfflinePinSettings,
}

impl Default for OperatorAuthSettings {
    fn default() -> Self {
        Self {
            // configuration.md: auth.operator.required_to_start = true
            required_to_start: true,
            // configuration.md: auth.operator.cached_session_hours = 24
            cached_session_hours: 24,
            offline_pin: OfflinePinSettings::default(),
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

// ── Разметка (метки/роли — фаза 2, этап 10) ──────────────────────────────────

/// `markers.*` — живая разметка заседания (этап 10). Роли интервалов
/// переиспользуют справочник `audio.roles`; здесь — только справочник категорий
/// закладок.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkersSettings {
    /// `markers.categories` — справочник категорий закладок (человекочитаемые
    /// ярлыки; значение уходит в W2.11 как подсказка категории). Настраивается
    /// заказчиком; дефолт — минимальный набор.
    #[serde(default = "default_marker_categories")]
    pub categories: Vec<String>,
}

impl Default for MarkersSettings {
    fn default() -> Self {
        Self {
            categories: default_marker_categories(),
        }
    }
}

/// Дефолтный справочник категорий закладок (`configuration.md` →
/// `markers.categories`) — согласованный с заказчиком минимальный набор.
fn default_marker_categories() -> Vec<String> {
    ["Закладка", "Инцидент", "Перерыв", "Прочее"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

// ── Проигрыватель (этап 10.1) ────────────────────────────────────────────────

/// `player.*` — встроенный проигрыватель сессий (этап 10.1). Дешифровка
/// потоковая, в памяти ядра; параметры — только навигация/скорость/частота
/// события позиции, никаких порогов шифрования (те — в `storage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerSettings {
    /// `player.seek_step_seconds` — шаг перемотки кнопками ±N (заседание идёт
    /// часами: мельче — избыточно кликов, крупнее — грубый промах мимо реплики).
    pub seek_step_seconds: f32,
    /// `player.playback_rates` — доступные скорости воспроизведения (0.5–2×,
    /// стандартный набор аудио/видео-плееров).
    pub playback_rates: Vec<f32>,
    /// `player.position_update_hz` — частота события позиции `player_position`
    /// для UI; ниже `audio.level_update_hz` — плейхеду не нужна живость метра.
    pub position_update_hz: u32,
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self {
            // configuration.md: player.seek_step_seconds = 15.0
            seek_step_seconds: 15.0,
            // configuration.md: player.playback_rates = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0]
            playback_rates: vec![0.5, 0.75, 1.0, 1.25, 1.5, 2.0],
            // configuration.md: player.position_update_hz = 5
            position_update_hz: 5,
        }
    }
}

// ── Экспорт (этап 10.2) ───────────────────────────────────────────────────────

/// Политика администратора для экспорта (`10.4`): единый параметр вместо
/// связки `enabled`+`require_confirmation` из кандидатов промта — исключает
/// противоречивую комбинацию и 1:1 ложится на трёхвариантный критерий
/// приёмки («экспорт разрешён/запрещён/только с подтверждением»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportPolicy {
    Allowed,
    Forbidden,
    RequiresConfirmation,
}

/// Формат аудио в экспортном пакете (не путать с `MasterCodec`/`ArchiveCodec` —
/// это выбор оператора в мастере экспорта, не параметр захвата/архива).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportCodec {
    WavPcm,
    Flac,
}

/// `export.*` — мастер экспорта записей (этап 10.2): состав/формат/назначение
/// пакета копии; сама сборка (дешифровка/склейка/FLAC/HTML-плеер/DVD) — без
/// параметров реестра (крипто- и Joliet-константы, не бизнес-логика).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportSettings {
    pub policy: ExportPolicy,
    pub default_codec: ExportCodec,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            // configuration.md: export.policy = allowed
            policy: ExportPolicy::Allowed,
            // configuration.md: export.default_codec = wav_pcm
            default_codec: ExportCodec::WavPcm,
        }
    }
}

// ── Администрирование: разграничение доступа (этап 10.4) ──────────────────────

/// `admin.pin` — политика админ-PIN (оффлайн-фолбэк прав администратора).
/// Сам PIN/его хеш в `settings.json` **не хранится** — только политика; хеш
/// Argon2id лежит в зашифрованном блобе `admin_pin.enc` (`store::admin_pin`),
/// задаётся при развёртывании через env `COURT_AUDIO_ADMIN_PIN`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminPinSettings {
    pub required: bool,
    pub min_length: u32,
}

impl Default for AdminPinSettings {
    fn default() -> Self {
        Self {
            // configuration.md: admin.pin.required = true
            required: true,
            // configuration.md: admin.pin.min_length = 4
            min_length: 4,
        }
    }
}

/// `admin.*` — разграничение доступа оператор/админ (этап 10.4). В v1 права
/// администратора = валидный админ-PIN; роль-из-`ex_system` (`admin.access`) —
/// отложена (открытый вопрос промта).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AdminSettings {
    pub pin: AdminPinSettings,
}

// ── Интерфейс и адаптивность (этап 10.5) ──────────────────────────────────────

/// `ui.hall_mode` — «режим зала» (крупный статус, читаемый с нескольких метров).
/// configuration.md: `ui.hall_mode.enabled = true`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HallModeSettings {
    pub enabled: bool,
}

impl Default for HallModeSettings {
    fn default() -> Self {
        // configuration.md: ui.hall_mode.enabled = true
        Self { enabled: true }
    }
}

/// `ui.compact_overlay` — компактное окно статуса «поверх всех окон» (Tauri
/// multi-window). configuration.md: `ui.compact_overlay.enabled = false` (опция).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CompactOverlaySettings {
    /// `Default` даёт `false` — оверлей выключен по умолчанию (опция зала).
    pub enabled: bool,
}

/// `ui.*` — параметры интерфейса и адаптивности (этап 10.5). Брейкпоинты сетки и
/// косметика метров — layout-константы кода (не бизнес-параметры реестра); здесь
/// только доступность режима зала и компакт-оверлея.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UiSettings {
    #[serde(default)]
    pub hall_mode: HallModeSettings,
    #[serde(default)]
    pub compact_overlay: CompactOverlaySettings,
}

// ── UX-пакет (этап 10.6) ──────────────────────────────────────────────────────

/// `ux.sound_alerts` — опциональный звуковой сигнал при сбоях (обрыв устройства /
/// критический диск / ошибка выгрузки). `Default` даёт `false`: в зале звук может
/// быть неуместен (этикет), оператор включает при необходимости. Дублирует, не
/// заменяет баннеры/трей.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SoundAlertsSettings {
    /// configuration.md: `ux.sound_alerts.enabled = false`.
    pub enabled: bool,
}

/// `ux.*` — UX-пакет (этап 10.6). Блокировка сна/автозапуск — на уровне ОС при
/// развёртывании (не параметры приложения, см. `docs/os_integration.md`); в
/// реестре — только опциональный звуковой сигнал.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UxSettings {
    #[serde(default)]
    pub sound_alerts: SoundAlertsSettings,
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
    /// Живая разметка (метки/роли — фаза 2, этап 10). `#[serde(default)]` на корне
    /// уже покрывает отсутствие ключа в конфигах v1/09.
    pub markers: MarkersSettings,
    /// Проигрыватель сессий (этап 10.1). `#[serde(default)]` на корне уже
    /// покрывает отсутствие ключа в старых конфигах.
    pub player: PlayerSettings,
    /// Мастер экспорта записей (этап 10.2). `#[serde(default)]` на корне уже
    /// покрывает отсутствие ключа в старых конфигах.
    pub export: ExportSettings,
    /// Разграничение доступа оператор/админ (этап 10.4). `#[serde(default)]` на
    /// корне уже покрывает отсутствие ключа в конфигах до 10.4.
    pub admin: AdminSettings,
    /// Интерфейс и адаптивность (этап 10.5). `#[serde(default)]` на корне уже
    /// покрывает отсутствие ключа в конфигах до 10.5.
    pub ui: UiSettings,
    /// UX-пакет (этап 10.6). `#[serde(default)]` на корне уже покрывает отсутствие
    /// ключа в конфигах до 10.6.
    pub ux: UxSettings,
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
        // Многоканал (фаза 2) — аддитивен, по умолчанию выключен.
        assert!(!s.audio.multichannel.enabled);
        assert!(s.audio.tracks.is_empty());
        assert_eq!(
            s.audio.roles,
            vec!["judge", "clerk", "prosecution", "defense", "witness", "room"]
        );
        assert_eq!(s.audio.sync.clock_master_track, 0);
        assert_eq!(s.audio.sync.drift_threshold_ms, 50);
        assert!(s.audio.sync.drift_compensate);
        assert!(!s.audio.master_downmix.enabled);
        // Разметка (фаза 2): справочник категорий — минимальный набор по умолчанию.
        assert_eq!(
            s.markers.categories,
            vec!["Закладка", "Инцидент", "Перерыв", "Прочее"]
        );
        // Проигрыватель (этап 10.1).
        assert_eq!(s.player.seek_step_seconds, 15.0);
        assert_eq!(s.player.playback_rates, vec![0.5, 0.75, 1.0, 1.25, 1.5, 2.0]);
        assert_eq!(s.player.position_update_hz, 5);
        // Экспорт (этап 10.2).
        assert_eq!(s.export.policy, ExportPolicy::Allowed);
        assert_eq!(s.export.default_codec, ExportCodec::WavPcm);
        // Аутентификация (этап 10.3).
        assert!(s.auth.station_identity.required);
        assert!(s.auth.operator.required_to_start);
        assert_eq!(s.auth.operator.cached_session_hours, 24);
        assert!(s.auth.operator.offline_pin.required);
        assert_eq!(s.auth.operator.offline_pin.min_length, 4);
        assert!(s.auth.recording_survives_token_expiry);
        // Администрирование (этап 10.4).
        assert!(s.admin.pin.required);
        assert_eq!(s.admin.pin.min_length, 4);
        // Интерфейс и адаптивность (этап 10.5).
        assert!(s.ui.hall_mode.enabled);
        assert!(!s.ui.compact_overlay.enabled);
        // UX-пакет (этап 10.6): звуковой сигнал по умолчанию выключен.
        assert!(!s.ux.sound_alerts.enabled);
    }

    #[test]
    fn pre_10_6_json_without_ux_key_loads_defaults() {
        // Конфиг до этапа 10.6 не содержит ключа `ux`: должен грузиться и
        // получать дефолты реестра (аддитивность через `#[serde(default)]`).
        let back: Settings = serde_json::from_str("{}").expect("deserialize empty");
        assert_eq!(back.ux, UxSettings::default());
        assert!(!back.ux.sound_alerts.enabled);
    }

    #[test]
    fn pre_10_5_json_without_ui_key_loads_defaults() {
        // Конфиг до этапа 10.5 не содержит ключа `ui`: должен грузиться и
        // получать дефолты реестра (аддитивность через `#[serde(default)]`).
        let back: Settings = serde_json::from_str("{}").expect("deserialize empty");
        assert_eq!(back.ui, UiSettings::default());
        assert!(back.ui.hall_mode.enabled);
        assert!(!back.ui.compact_overlay.enabled);
    }

    #[test]
    fn pre_10_4_json_without_admin_key_loads_defaults() {
        // Конфиг до этапа 10.4 не содержит ключа `admin`: должен грузиться и
        // получать дефолты реестра (аддитивность через `#[serde(default)]`).
        let back: Settings = serde_json::from_str("{}").expect("deserialize empty");
        assert_eq!(back.admin, AdminSettings::default());
        assert!(back.admin.pin.required);
    }

    #[test]
    fn pre_10_3_auth_json_without_offline_pin_loads_defaults() {
        // Конфиг до этапа 10.3 не содержит ключа `offline_pin`: должен грузиться и
        // получать дефолты реестра (аддитивность через `#[serde(default)]`).
        let pre = r#"{"auth":{"station_identity":{"required":true},
            "operator":{"required_to_start":true,"cached_session_hours":24},
            "recording_survives_token_expiry":true}}"#;
        let s: Settings = serde_json::from_str(pre).expect("pre-10.3 auth загружается");
        assert_eq!(s.auth.operator.offline_pin, OfflinePinSettings::default());
        assert_eq!(s.auth, Settings::default().auth);
    }

    #[test]
    fn v1_json_without_export_key_loads_with_defaults() {
        // Конфиг до этапа 10.2 не содержит ключа `export`: должен грузиться и
        // получать дефолты реестра (аддитивность через `#[serde(default)]`).
        let back: Settings = serde_json::from_str("{}").expect("deserialize empty");
        assert_eq!(back.export, ExportSettings::default());
    }

    #[test]
    fn export_policy_and_codec_serialize_snake_case() {
        let s = ExportSettings {
            policy: ExportPolicy::RequiresConfirmation,
            default_codec: ExportCodec::Flac,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"policy\":\"requires_confirmation\""));
        assert!(json.contains("\"default_codec\":\"flac\""));
        let back: ExportSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn v1_audio_json_without_multichannel_keys_loads() {
        // Конфиг v1 не содержит ключей многоканала: должен грузиться и получать
        // одноканальные дефолты (аддитивность).
        let v1 = r#"{"audio":{"device":null,"sample_rate_hz":44100,"bit_depth":16,
            "channels":1,"master_codec":"wav_pcm","capture_buffer_seconds":2.0,
            "level_update_hz":25,"archive_copy":{"enabled":false,"codec":"flac"}}}"#;
        let s: Settings = serde_json::from_str(v1).expect("v1 audio загружается");
        assert!(!s.audio.multichannel.enabled);
        assert!(s.audio.tracks.is_empty());
        assert_eq!(s.audio.roles.len(), 6);
        assert_eq!(s.audio, Settings::default().audio);
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
