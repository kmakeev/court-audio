// Общие хелперы отображения записи (этап 10.5). Раньше были приватными в экране
// «Запись»; вынесены, чтобы шапка, «режим зала» и компакт-оверлей показывали
// хронометр/уровень одинаково. Косметика метра — layout-константы, не реестр.

// Полная шкала нормированного PCM-уровня: клиппинг при пике у предела сигнала.
export const FULL_SCALE = 1.0;
// Порог визуальной индикации клиппинга (доля от полной шкалы) — косметика метра.
export const CLIP_RATIO = 0.99;
// Нижняя граница шкалы метра в дБFS: уровень меряем логарифмически (как все
// аудио-метры), иначе тихая речь в линейной шкале выглядит «мёртвой». Косметика.
export const METER_FLOOR_DBFS = -60;

/** Хронометр `ЧЧ:ММ:СС` из целого числа секунд. */
export function formatClock(totalSec: number): string {
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(h)}:${pad(m)}:${pad(s)}`;
}

/**
 * Преобразование линейной амплитуды [0..1] в проценты шкалы по дБFS: тихая речь
 * в линейной шкале почти не видна, логарифм делает метр «живым».
 */
export function toMeterPct(v: number): number {
  if (v <= 0) return 0;
  const db = 20 * Math.log10(v); // dBFS, ≤ 0
  const pct = ((db - METER_FLOOR_DBFS) / (0 - METER_FLOOR_DBFS)) * 100;
  return Math.max(0, Math.min(100, pct));
}

/** Одно событие уровня: массив каналов с пиком/RMS (зеркало `LevelEvent`). */
interface LevelLike {
  channels: { peak: number; rms: number }[];
}

/**
 * Сводный уровень по всем дорожкам/каналам — для крупного метра «режима зала» и
 * компакт-оверлея (там достаточно одного столбика: берём максимум RMS/пика).
 */
export function aggregateLevel(levels: Record<number, LevelLike>): {
  rms: number;
  peak: number;
} {
  let rms = 0;
  let peak = 0;
  for (const ev of Object.values(levels)) {
    for (const ch of ev.channels) {
      if (ch.rms > rms) rms = ch.rms;
      if (ch.peak > peak) peak = ch.peak;
    }
  }
  return { rms, peak };
}
