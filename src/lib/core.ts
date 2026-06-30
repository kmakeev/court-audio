// Тонкий типизированный слой над Tauri-командами и событиями ядра захвата
// (этапы 01–03). UI не дублирует бизнес-логику — только вызывает команды и
// слушает события; типы здесь — TS-зеркало Rust-структур из
// `src-tauri/src/ipc/*` и `src-tauri/src/store/manifest.rs`.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

// ── Устройства (audio::devices::DeviceInfo) ─────────────────────────────────

export interface ConfigRange {
  channels: number;
  min_sample_rate_hz: number;
  max_sample_rate_hz: number;
  sample_format: string;
}

export interface DeviceInfo {
  name: string;
  is_default: boolean;
  default_sample_rate_hz: number | null;
  default_channels: number | null;
  configs: ConfigRange[];
}

// ── События ядра ────────────────────────────────────────────────────────────

/** `audio_level` — нормированный уровень `[0.0, 1.0]` (capture::LevelEvent). */
export interface LevelEvent {
  peak: number;
  rms: number;
}

/** Состояние конвейера захвата (capture_state). */
export type CaptureStateValue =
  | 'idle'
  | 'recording'
  | 'paused'
  | 'stopping'
  | 'stopped';

export interface CaptureStateEvent {
  state: CaptureStateValue;
}

/** `reliability_warning` (capture::ReliabilityEvent), тег `kind` (snake_case). */
export type ReliabilityEvent =
  | { kind: 'disk_low'; free_mb: number }
  | { kind: 'disk_critical'; free_mb: number }
  | { kind: 'watchdog_restart' }
  | { kind: 'device_lost' }
  | { kind: 'device_back' }
  | { kind: 'max_duration_warning' };

// ── Команды захвата (ipc::audio_cmds) ───────────────────────────────────────

export interface CaptureStarted {
  sample_rate_hz: number;
  channels: number;
  output_dir: string;
}

export interface SegmentSummary {
  index: number;
  path: string;
  frames: number;
  /** u128 сериализуется строкой (см. Rust). */
  started_at_unix_ms: string;
}

export interface RecoverableSession {
  dir: string;
  completed_segments: number;
  already_recovered: boolean;
}

export function listAudioDevices(): Promise<DeviceInfo[]> {
  return invoke<DeviceInfo[]>('list_audio_devices');
}

export function startCapture(): Promise<CaptureStarted> {
  return invoke<CaptureStarted>('start_capture');
}

export function stopCapture(): Promise<SegmentSummary[]> {
  return invoke<SegmentSummary[]>('stop_capture');
}

export function pauseCapture(): Promise<void> {
  return invoke('pause_capture');
}

export function resumeCapture(): Promise<void> {
  return invoke('resume_capture');
}

export function scanRecoverable(): Promise<RecoverableSession[]> {
  return invoke<RecoverableSession[]>('scan_recoverable');
}

export function recoverSession(dir: string): Promise<void> {
  return invoke('recover_session', { dir });
}

export function discardSession(dir: string): Promise<void> {
  return invoke('discard_session', { dir });
}

// ── Манифест сессий (store::manifest) ───────────────────────────────────────

export type SessionStatus = 'recording' | 'stopped' | 'recovered' | 'purged';

export type UploadStatus =
  | 'pending'
  | 'uploading'
  | 'uploaded'
  | 'confirmed'
  | 'failed';

export type EventKind =
  | 'session_started'
  | 'segment_rotated'
  | 'paused'
  | 'resumed'
  | 'device_lost'
  | 'device_back'
  | 'recovered'
  | 'stopped';

export interface SessionRecord {
  id: string;
  dir: string;
  started_at_unix_ms: number;
  status: SessionStatus;
  station_id: string;
  operator_id: string;
  adjudication_ref: string | null;
  sample_rate_hz: number;
  channels: number;
  bit_depth: number;
  final_chain_link: string | null;
  upload_status: UploadStatus;
  server_integrity_verified: boolean;
  confirmed_at_unix_ms: number | null;
  local_purged_at_unix_ms: number | null;
}

export interface EventRecord {
  seq: number;
  kind: EventKind;
  at_unix_ms: number;
  detail?: unknown;
}

/** SessionRecord + производные (длительность, число сегментов) для списка. */
export interface SessionView extends SessionRecord {
  segment_count: number;
  duration_seconds: number;
}

export function listSessions(): Promise<SessionView[]> {
  return invoke<SessionView[]>('list_sessions');
}

// ── Диагностика (ipc::query_cmds::DiagnosticsInfo) ──────────────────────────

export type DiskStatusCode = 'ok' | 'low' | 'critical';

export interface DiskInfo {
  free_mb: number;
  status: DiskStatusCode;
  low_threshold_mb: number;
  critical_mb: number;
}

export interface IntegritySummary {
  session_id: string;
  segments: number;
  segments_hashed: number;
  final_chain_link: string | null;
  hash_chain_enabled: boolean;
  event_log_enabled: boolean;
}

export interface StationInfo {
  app_version: string;
  storage_root: string;
  station_id: string | null;
}

export interface DiagnosticsInfo {
  devices: DeviceInfo[];
  disk: DiskInfo;
  station: StationInfo;
  last_session: SessionRecord | null;
  recent_events: EventRecord[];
  integrity: IntegritySummary | null;
}

export function getDiagnostics(): Promise<DiagnosticsInfo> {
  return invoke<DiagnosticsInfo>('diagnostics');
}

// ── Типизированные подписки на события ───────────────────────────────────────

export function onAudioLevel(cb: (e: LevelEvent) => void): Promise<UnlistenFn> {
  return listen<LevelEvent>('audio_level', (ev) => cb(ev.payload));
}

export function onCaptureState(
  cb: (e: CaptureStateEvent) => void,
): Promise<UnlistenFn> {
  return listen<CaptureStateEvent>('capture_state', (ev) => cb(ev.payload));
}

export function onReliabilityWarning(
  cb: (e: ReliabilityEvent) => void,
): Promise<UnlistenFn> {
  return listen<ReliabilityEvent>('reliability_warning', (ev) => cb(ev.payload));
}
