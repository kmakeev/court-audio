import type { CSSProperties } from 'react';

/**
 * Полоса прогресса в стиле дизайн-системы (создана локально по аналогии с
 * `Skeleton.tsx`/`Checkbox.tsx`). Два режима:
 * - **индетерминированный** (по умолчанию, без `value`): бегущий сегмент —
 *   «идёт работа, длительность неизвестна» (напр. финализация записи);
 * - **детерминированный** (`value` 0..100): заполнение по доле.
 *
 * Токены ДС: дорожка — `--paper-strong` + `--hairline`, заливка — `--accent`.
 * Анимация уважает `prefers-reduced-motion` (см. `styles/globals.css`).
 */
interface ProgressBarProps {
  /** Подпись для скринридера (`role="progressbar"`). */
  label: string;
  /** Доля прогресса 0..100. Без значения — индетерминированный режим. */
  value?: number;
  style?: CSSProperties;
}

export function ProgressBar({ label, value, style }: ProgressBarProps) {
  const indeterminate = value == null;
  const clamped = indeterminate ? 0 : Math.max(0, Math.min(100, value));
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={indeterminate ? undefined : 0}
      aria-valuemax={indeterminate ? undefined : 100}
      aria-valuenow={indeterminate ? undefined : Math.round(clamped)}
      style={{
        position: 'relative',
        width: '100%',
        height: 6,
        background: 'var(--paper-strong)',
        border: '1px solid var(--hairline)',
        overflow: 'hidden',
        ...style,
      }}
    >
      <div
        className={indeterminate ? 'progressbar-indeterminate' : undefined}
        aria-hidden="true"
        style={
          indeterminate
            ? { position: 'absolute', top: 0, bottom: 0, width: '40%', background: 'var(--accent)' }
            : {
                position: 'absolute',
                top: 0,
                bottom: 0,
                left: 0,
                width: `${clamped}%`,
                background: 'var(--accent)',
                transition: 'width 200ms ease',
              }
        }
      />
    </div>
  );
}
