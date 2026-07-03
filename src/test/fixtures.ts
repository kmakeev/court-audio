// Тестовые фикстуры (синтетика, без реальных ПДн — правило CLAUDE.md).
import type { Settings } from '../lib/settings';
import type {
  AuthStatus,
  DiagnosticsInfo,
  ExportResult,
  ExportSessionInfo,
  PlayerSessionInfo,
  SessionView,
} from '../lib/core';

/** Полный объект `Settings` с валидными значениями (зеркало реестра-дефолтов). */
export function settingsFixture(): Settings {
  return {
    audio: {
      device: null,
      sample_rate_hz: 44100,
      bit_depth: 16,
      channels: 1,
      master_codec: 'wav_pcm',
      archive_copy: { enabled: false, codec: 'flac' },
      multichannel: { enabled: false },
      tracks: [],
      roles: ['judge', 'clerk', 'prosecution', 'defense', 'witness', 'room'],
      sync: { clock_master_track: 0, drift_threshold_ms: 50, drift_compensate: true },
      master_downmix: { enabled: false },
    },
    recorder: { segment_seconds: 30, flush_interval_ms: 1500, max_session_hours: 12 },
    reliability: {
      watchdog_timeout_ms: 5000,
      disk_low_threshold_mb: 1024,
      disk_critical_mb: 256,
      device_reconnect: { auto_resume: true },
      mirror: { enabled: false, path: null },
    },
    storage: { root_path: null, encrypt_at_rest: true },
    retention: {
      mode: 'until_confirmed_plus_window',
      require_integrity_verified: true,
      safety_window_hours: 72,
    },
    integrity: { segment_hash: 'sha256', hash_chain: true, event_log: true, gost_sign: false },
    sync: {
      server_base_url: 'https://ex.example',
      chunk_size_mb: 8,
      parallel_uploads: 1,
      auto_upload: true,
      defer_during_recording: false,
      offline_queue: { enabled: true },
      retry: { backoff_base_ms: 2000, backoff_max_ms: 60000, max_attempts: 0 },
    },
    auth: {
      station_identity: { required: true },
      operator: {
        required_to_start: true,
        cached_session_hours: 24,
        offline_pin: { required: true, min_length: 4 },
      },
      recording_survives_token_expiry: true,
    },
    case_cache: {
      enabled: true,
      encrypt: true,
      scope: 'court_docket',
      ttl_hours: 24,
      max_records: 500,
    },
    markers: {
      categories: ['Закладка', 'Инцидент', 'Перерыв', 'Прочее'],
    },
    player: {
      seek_step_seconds: 15,
      playback_rates: [0.5, 0.75, 1.0, 1.25, 1.5, 2.0],
      position_update_hz: 5,
    },
    export: {
      policy: 'allowed',
      default_codec: 'wav_pcm',
    },
    admin: {
      pin: { required: true, min_length: 4 },
    },
  };
}

/** Статус аутентификации (этап 10.3) — по умолчанию оператор вошёл онлайн. */
export function authStatusFixture(over: Partial<AuthStatus> = {}): AuthStatus {
  return {
    operator: {
      operator_id: '42',
      full_name: 'Иванов И. И.',
      role: 'assistant',
    },
    online: true,
    offline_cached: false,
    cache_expires_at_unix_ms: 1_700_086_400_000,
    pin_required: true,
    ...over,
  };
}

export function sessionViewFixture(over: Partial<SessionView> = {}): SessionView {
  return {
    id: 'session-1700000000000',
    dir: '/data/recordings/session-1700000000000',
    started_at_unix_ms: 1_700_000_000_000,
    status: 'stopped',
    station_id: 'station-A',
    operator_id: 'op-1',
    adjudication_ref: '№ 1-123/2026, Иванов И.И.',
    sample_rate_hz: 44100,
    channels: 1,
    bit_depth: 16,
    final_chain_link: 'abc123',
    upload_status: 'pending',
    server_integrity_verified: false,
    confirmed_at_unix_ms: null,
    local_purged_at_unix_ms: null,
    upload_paused: false,
    segment_count: 4,
    duration_seconds: 125,
    upload_total_parts: 0,
    upload_sent_parts: 0,
    ...over,
  };
}

/** Ответ `player_open_session` (этап 10.1) с одной меткой и одним интервалом роли. */
export function playerSessionInfoFixture(
  over: Partial<PlayerSessionInfo> = {},
): PlayerSessionInfo {
  return {
    session_id: 'session-1700000000000',
    started_at_unix_ms: 1_700_000_000_000,
    adjudication_ref: '№ 1-123/2026, Иванов И.И.',
    tracks: [{ track_id: 0, role: 'single', label: 'Запись' }],
    markers: [
      {
        id: 'm1',
        category: 'Инцидент',
        comment: 'шум в зале',
        offset_samples: 44_100,
        offset_ms: 1_000,
        operator_id: 'op-1',
        at_unix_ms: 1_700_000_001_000,
      },
    ],
    role_spans: [
      {
        id: 'r1',
        role: 'judge',
        start_offset_samples: 88_200,
        start_offset_ms: 2_000,
        end_offset_samples: 132_300,
        end_offset_ms: 3_000,
        operator_id: 'op-1',
        at_unix_ms: 1_700_000_002_000,
      },
    ],
    duration_ms: 125_000,
    sample_rate_hz: 44_100,
    integrity_ok: true,
    ...over,
  };
}

/** Ответ `export_session_info` (этап 10.2) — сессия с двумя дорожками. */
export function exportSessionInfoFixture(
  over: Partial<ExportSessionInfo> = {},
): ExportSessionInfo {
  return {
    session_id: 'session-1700000000000',
    adjudication_ref: '№ 1-123/2026, Иванов И.И.',
    started_at_unix_ms: 1_700_000_000_000,
    duration_ms: 125_000,
    tracks: [
      { track_id: 0, role: 'judge', label: 'Судья' },
      { track_id: 1, role: 'defense', label: 'Защита' },
    ],
    integrity_ok: true,
    ...over,
  };
}

/** Ответ `export_build_package` (этап 10.2). */
export function exportResultFixture(over: Partial<ExportResult> = {}): ExportResult {
  return {
    package_dir: '/data/export/1-123_2026_2026-07-02',
    files: [
      { name: 'audio/sudya.wav', sha256: 'a'.repeat(64), size_bytes: 123_456 },
      { name: 'audio/zaschita.wav', sha256: 'b'.repeat(64), size_bytes: 111_222 },
    ],
    manifest_path: '/data/export/1-123_2026_2026-07-02/manifest.json',
    player_path: '/data/export/1-123_2026_2026-07-02/player.html',
    ...over,
  };
}

export function diagnosticsFixture(over: Partial<DiagnosticsInfo> = {}): DiagnosticsInfo {
  return {
    devices: [
      {
        name: 'Микрофон зала',
        is_default: true,
        default_sample_rate_hz: 44100,
        default_channels: 1,
        configs: [],
      },
    ],
    disk: { free_mb: 5000, status: 'ok', low_threshold_mb: 1024, critical_mb: 256 },
    station: {
      app_version: '0.1.0',
      storage_root: '/data/recordings',
      station_id: 'station-A',
    },
    last_session: null,
    recent_events: [
      { seq: 1, kind: 'session_started', at_unix_ms: 1_700_000_000_000 },
      { seq: 2, kind: 'stopped', at_unix_ms: 1_700_000_125_000 },
    ],
    integrity: {
      session_id: 'session-1700000000000',
      segments: 4,
      segments_hashed: 4,
      final_chain_link: 'abc123',
      hash_chain_enabled: true,
      event_log_enabled: true,
    },
    ...over,
  };
}
