// TS-зеркало модели `Settings` из `src-tauri/src/settings.rs` и обёртки над
// IPC-командами. Значения по умолчанию задаёт Rust (реестр
// `docs/configuration.md`); фронтенд их только читает и сохраняет.
import { invoke } from '@tauri-apps/api/core';

export type MasterCodec = 'wav_pcm' | 'flac';
export type ArchiveCodec = 'flac';
export type RetentionMode =
  | 'until_confirmed_plus_window'
  | 'delete_on_confirm'
  | 'manual';

/** Одна дорожка карты «канал ↔ роль» (`audio.tracks[*]`, фаза 2 — этап 09). */
export interface TrackConfig {
  device: string | null;
  channel_index: number;
  role: string;
  label: string;
}

export interface AudioSettings {
  device: string | null;
  sample_rate_hz: number;
  bit_depth: number;
  channels: number;
  master_codec: MasterCodec;
  archive_copy: { enabled: boolean; codec: ArchiveCodec };
  // Многоканал по ролям (фаза 2). Аддитивно: по умолчанию выключено.
  multichannel: { enabled: boolean };
  tracks: TrackConfig[];
  roles: string[];
  sync: {
    clock_master_track: number;
    drift_threshold_ms: number;
    drift_compensate: boolean;
  };
  master_downmix: { enabled: boolean };
}

export interface RecorderSettings {
  segment_seconds: number;
  flush_interval_ms: number;
  max_session_hours: number;
}

export interface ReliabilitySettings {
  watchdog_timeout_ms: number;
  disk_low_threshold_mb: number;
  disk_critical_mb: number;
  device_reconnect: { auto_resume: boolean };
  mirror: { enabled: boolean; path: string | null };
}

export interface StorageSettings {
  root_path: string | null;
  encrypt_at_rest: boolean;
}

export interface RetentionSettings {
  mode: RetentionMode;
  require_integrity_verified: boolean;
  safety_window_hours: number;
}

export interface IntegritySettings {
  segment_hash: string;
  hash_chain: boolean;
  event_log: boolean;
  gost_sign: boolean;
}

export interface SyncSettings {
  server_base_url: string | null;
  chunk_size_mb: number;
  parallel_uploads: number;
  auto_upload: boolean;
  defer_during_recording: boolean;
  offline_queue: { enabled: boolean };
  retry: {
    backoff_base_ms: number;
    backoff_max_ms: number;
    max_attempts: number;
  };
}

export interface AuthSettings {
  station_identity: { required: boolean };
  operator: {
    required_to_start: boolean;
    cached_session_hours: number;
    // Этап 10.3: PIN как второй фактор оффлайн-старта по кэшу.
    offline_pin: { required: boolean; min_length: number };
  };
  recording_survives_token_expiry: boolean;
}

export interface CaseCacheSettings {
  enabled: boolean;
  encrypt: boolean;
  scope: string;
  ttl_hours: number;
  max_records: number;
}

/** Живая разметка (метки/роли — фаза 2, этап 10). Роли берутся из `audio.roles`. */
export interface MarkersSettings {
  categories: string[];
}

/** Проигрыватель сессий (этап 10.1). */
export interface PlayerSettings {
  seek_step_seconds: number;
  playback_rates: number[];
  position_update_hz: number;
}

/** Политика администратора для экспорта (этап 10.2). */
export type ExportPolicy = 'allowed' | 'forbidden' | 'requires_confirmation';
export type ExportCodec = 'wav_pcm' | 'flac';

/** Мастер экспорта записей (этап 10.2). */
export interface ExportSettings {
  policy: ExportPolicy;
  default_codec: ExportCodec;
}

/** Разграничение доступа оператор/админ (этап 10.4). */
export interface AdminSettings {
  pin: { required: boolean; min_length: number };
}

/** Интерфейс и адаптивность (этап 10.5). Брейкпоинты — layout-константы кода. */
export interface UiSettings {
  /** Доступность «режима зала» (крупный статус). */
  hall_mode: { enabled: boolean };
  /** Доступность компакт-окна статуса «поверх всех окон». */
  compact_overlay: { enabled: boolean };
}

export interface Settings {
  audio: AudioSettings;
  recorder: RecorderSettings;
  reliability: ReliabilitySettings;
  storage: StorageSettings;
  retention: RetentionSettings;
  integrity: IntegritySettings;
  sync: SyncSettings;
  auth: AuthSettings;
  case_cache: CaseCacheSettings;
  markers: MarkersSettings;
  player: PlayerSettings;
  export: ExportSettings;
  admin: AdminSettings;
  ui: UiSettings;
}

// ── Разграничение доступа (этап 10.4: ipc::admin_cmds) ───────────────────────

/** Итог сохранения настроек (admin_cmds::SaveOutcome, тег `kind`). */
export type SaveOutcome =
  | { kind: 'saved' }
  | { kind: 'needs_confirmation'; dangerous: string[] };

/** Статус админ-доступа (admin_cmds::AdminStatusView). */
export interface AdminStatus {
  /** Задан ли админ-PIN при развёртывании (иначе админ-изменения невозможны). */
  provisioned: boolean;
  /** Разблокирован ли админ-доступ в текущем сеансе. */
  unlocked: boolean;
  /** Требуется ли админ-PIN политикой (`admin.pin.required`). */
  required: boolean;
}

/** Одно изменённое поле в журнале настроек (store::settings_audit::FieldChange). */
export interface SettingsFieldChange {
  path: string;
  old: unknown;
  new: unknown;
}

/** Источник изменения настроек (store::settings_audit::ChangeSource). */
export type ChangeSource = 'manual' | 'import';

/** Запись журнала изменений настроек (store::settings_audit::SettingsAuditRecord). */
export interface SettingsAuditRecord {
  seq: number;
  at_unix_ms: number;
  actor_operator_id: string;
  source: ChangeSource;
  dangerous: boolean;
  changes: SettingsFieldChange[];
}

/** Прочитать настройки (Rust возвращает дефолты из реестра при отсутствии файла). */
export function getSettings(): Promise<Settings> {
  return invoke<Settings>('get_settings');
}

/**
 * Сохранить настройки (этап 10.4). Гейт оператор/админ — на уровне ядра:
 * админ-изменение без прав отклоняется (`Err`), опасное изменение без
 * `confirmDangerous` возвращает `needs_confirmation` (диалог подтверждения).
 */
export function saveSettings(
  settings: Settings,
  confirmDangerous = false,
): Promise<SaveOutcome> {
  return invoke<SaveOutcome>('save_settings', { settings, confirmDangerous });
}

/** Статус админ-доступа (задан ли PIN, разблокирован ли сеанс). */
export function adminStatus(): Promise<AdminStatus> {
  return invoke<AdminStatus>('admin_status');
}

/** Разблокировать админ-доступ по PIN (неверный PIN → `Err`). */
export function adminUnlock(pin: string): Promise<AdminStatus> {
  return invoke<AdminStatus>('admin_unlock', { pin });
}

/** Заблокировать админ-доступ (снять разблокировку сеанса). */
export function adminLock(): Promise<AdminStatus> {
  return invoke<AdminStatus>('admin_lock');
}

/** Экспорт профиля станции: полный `Settings` в JSON (без секретов). */
export function exportStationProfile(): Promise<string> {
  return invoke<string>('export_station_profile');
}

/** Импорт профиля станции (только администратором, журналируется). */
export function importStationProfile(
  profileJson: string,
  confirmDangerous = false,
): Promise<SaveOutcome> {
  return invoke<SaveOutcome>('import_station_profile', { profileJson, confirmDangerous });
}

/** Журнал изменений настроек (новейшие сверху, не более `limit`). */
export function getSettingsAudit(limit: number): Promise<SettingsAuditRecord[]> {
  return invoke<SettingsAuditRecord[]>('get_settings_audit', { limit });
}
