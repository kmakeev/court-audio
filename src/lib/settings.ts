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
  operator: { required_to_start: boolean; cached_session_hours: number };
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
}

/** Прочитать настройки (Rust возвращает дефолты из реестра при отсутствии файла). */
export function getSettings(): Promise<Settings> {
  return invoke<Settings>('get_settings');
}

/** Сохранить настройки в файл конфигурации Tauri. */
export function saveSettings(settings: Settings): Promise<void> {
  return invoke('save_settings', { settings });
}
