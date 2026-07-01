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

/** Уровень одного канала (нормированный `[0.0, 1.0]`). */
export interface ChannelLevel {
  peak: number;
  rms: number;
}

/**
 * `audio_level` — уровни по каналам (capture::LevelEvent). `track_id` относит
 * событие к дорожке (многоканал — этап 09; для v1 всегда 0).
 */
export interface LevelEvent {
  track_id?: number;
  channels: ChannelLevel[];
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

/** Запустить мониторинг уровня без записи (живой индикатор «микрофон работает»). */
export function startMonitor(): Promise<void> {
  return invoke('start_monitor');
}

/** Остановить мониторинг уровня и освободить устройство. */
export function stopMonitor(): Promise<void> {
  return invoke('stop_monitor');
}

/** Текущее состояние захвата для восстановления статуса UI после перехода. */
export interface CaptureStatus {
  state: 'idle' | 'recording' | 'paused';
  started_at_unix_ms: number | null;
  output_dir: string | null;
  segment_count: number;
}

export function getCaptureStatus(): Promise<CaptureStatus> {
  return invoke<CaptureStatus>('capture_status');
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
  | 'failed'
  | 'integrity_failed';

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
  /** Операторская пауза догрузки (этап 06). */
  upload_paused: boolean;
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
  /** Всего частей выгрузки (= сегментов); 0 — выгрузка не начиналась (этап 06). */
  upload_total_parts: number;
  /** Сколько частей принято сервером (для прогресса «выгружается N%»). */
  upload_sent_parts: number;
}

export function listSessions(): Promise<SessionView[]> {
  return invoke<SessionView[]>('list_sessions');
}

// ── Управление выгрузкой (этап 06: ipc::sync_cmds) ──────────────────────────

/** Повторить выгрузку записи: сбросить ошибку → в очередь, снять паузу. */
export function retryUpload(dir: string): Promise<void> {
  return invoke('retry_upload', { dir });
}

/** Поставить выгрузку записи на паузу (планировщик её пропускает). */
export function pauseUpload(dir: string): Promise<void> {
  return invoke('pause_upload', { dir });
}

/** Снять паузу выгрузки записи. */
export function resumeUpload(dir: string): Promise<void> {
  return invoke('resume_upload', { dir });
}

// ── Привязка к делу (этап 05: store::case_binding / store::case_cache) ──────

/** Состояние привязки (store::case_binding::BindingKind). */
export type BindingKind = 'resolved' | 'manual';

/**
 * Привязка записи к делу. `resolved` — выбрано дело из кэша (есть
 * `adjudication_id`); `manual` (pending) — ручной ввод № (+опц. ФИО), сервер
 * свяжет позже. Сериализуется в JSON и пишется в `adjudication_ref` манифеста.
 */
export interface AdjudicationRef {
  kind: BindingKind;
  adjudication_id?: string;
  raw_number?: string;
  raw_fio?: string;
}

/** Дело из кэша докета (store::case_cache::CaseRecord) — минимум полей ПДн. */
export interface CaseRecord {
  id: string;
  number: string;
  fio: string;
  date: string;
}

/** Свежесть/объём кэша дел (ipc::case_cmds::CaseCacheStatus). */
export interface CaseCacheStatus {
  synced_at_unix_ms: number | null;
  is_fresh: boolean;
  record_count: number;
  scope: string;
}

/** Оффлайн-поиск дел в локальном кэше (автокомплит пикера). */
export function searchCases(query: string): Promise<CaseRecord[]> {
  return invoke<CaseRecord[]>('search_cases', { query });
}

/** Свежесть и объём кэша дел для индикатора пикера. */
export function getCaseCacheStatus(): Promise<CaseCacheStatus> {
  return invoke<CaseCacheStatus>('get_case_cache_status');
}

/**
 * Синхронизировать кэш дел из докета `ex_system`. В этапе 05 транспорта ещё нет
 * (HTTP — 06, slim-эндпоинт/авторизация — 07): команда отклоняется с пояснением.
 */
export function syncCaseCache(): Promise<CaseCacheStatus> {
  return invoke<CaseCacheStatus>('sync_case_cache');
}

/** Привязать/уточнить/снять (`null`) дело у записи в каталоге `dir`. */
export function bindSessionCase(
  dir: string,
  binding: AdjudicationRef | null,
): Promise<void> {
  return invoke('bind_session_case', { dir, binding });
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
