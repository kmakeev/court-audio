// Опциональный звуковой сигнал о сбоях (этап 10.6, deliverable 5). Дублирует, не
// заменяет баннеры/трей: оператор, отвернувшись, не пропустит проблему. Гейт —
// `settings.ux.sound_alerts.enabled` (дефолт false: в зале звук может быть
// неуместен). Звук — Web Audio в webview (без плагина ОС). Решение «бипать ли» —
// чистая `shouldBeep` (тестируема); сам `beep` защищён от отсутствия AudioContext.

import type { ReliabilityEvent } from './core';

/** Класс события, на который может реагировать звуковой сигнал. */
export type AlertEvent =
  | { kind: 'device_lost' }
  | { kind: 'disk_critical' }
  | { kind: 'upload_failed' };

/**
 * Нужно ли подавать звуковой сигнал: только при включённой опции и только на
 * значимые сбои (обрыв устройства, критический диск, ошибка выгрузки). Возврат
 * устройства/предупреждение «мало места»/watchdog-рестарт звуком не тревожат.
 */
export function shouldBeep(soundAlertsEnabled: boolean, event: AlertEvent): boolean {
  if (!soundAlertsEnabled) return false;
  switch (event.kind) {
    case 'device_lost':
    case 'disk_critical':
    case 'upload_failed':
      return true;
  }
}

/**
 * Сопоставить событие надёжности ядра (`ReliabilityEvent`) классу алерта, если оно
 * тревожное. `null` — событие не требует звукового сигнала (возврат устройства,
 * «мало места» без критики, watchdog-рестарт, предупреждение о длительности).
 */
export function reliabilityToAlert(e: ReliabilityEvent): AlertEvent | null {
  switch (e.kind) {
    case 'device_lost':
      return { kind: 'device_lost' };
    case 'disk_critical':
      return { kind: 'disk_critical' };
    default:
      return null;
  }
}

// Единый AudioContext на приложение (создаётся лениво при первом сигнале).
let ctx: AudioContext | null = null;

/** Тип-хелпер: конструктор AudioContext может быть под префиксом webkit. */
type AudioCtor = typeof AudioContext;

function audioContext(): AudioContext | null {
  const w = window as unknown as { AudioContext?: AudioCtor; webkitAudioContext?: AudioCtor };
  const Ctor = w.AudioContext ?? w.webkitAudioContext;
  if (!Ctor) return null; // среда без Web Audio (jsdom/старый webview) — тихо выходим
  if (!ctx) ctx = new Ctor();
  return ctx;
}

/**
 * Короткий двойной бип (внимание к сбою). Best-effort: отсутствие Web Audio или
 * ошибка воспроизведения не должны валить UI. Громкость умеренная — сигнал, не
 * тревога.
 */
export function beep(): void {
  const ac = audioContext();
  if (!ac) return;
  try {
    const now = ac.currentTime;
    for (const offset of [0, 0.18]) {
      const osc = ac.createOscillator();
      const gain = ac.createGain();
      osc.type = 'sine';
      osc.frequency.value = 880;
      gain.gain.setValueAtTime(0.0001, now + offset);
      gain.gain.exponentialRampToValueAtTime(0.15, now + offset + 0.01);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + offset + 0.12);
      osc.connect(gain).connect(ac.destination);
      osc.start(now + offset);
      osc.stop(now + offset + 0.13);
    }
  } catch {
    // Аудио недоступно/заблокировано автоплей-политикой — игнорируем.
  }
}

/** Подать сигнал, если опция включена и событие тревожное (комбинация выше). */
export function maybeBeep(soundAlertsEnabled: boolean, event: AlertEvent): void {
  if (shouldBeep(soundAlertsEnabled, event)) beep();
}
