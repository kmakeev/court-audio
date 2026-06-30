// Раунд 06.4.c (2026-05-30): блокирующий/предупреждающий баннер для
// ситуаций, требующих внимания судьи (исходно — ст. 20 УК РФ:
// лицо НЕ подлежит уголовной ответственности).
//
// Два варианта:
// - `critical` (default) — тёмно-красный баннер на токенах `--accent-deep`
//   (`forbidden`-уровень, обязательное действие судьи);
// - `warning` — мягкий жёлтый/янтарный (агрегатный summary, информирует
//   о наличии проблемных эпизодов у подсудимого).
//
// Не блокирует расчёт/экспорт по решению заказчика (раунд 06.4.c Q1).

import { Link } from 'react-router-dom';
import type { CSSProperties, ReactNode } from 'react';

type Variant = 'critical' | 'warning';

interface CriticalNoticeProps {
  title: string;
  description?: ReactNode;
  variant?: Variant;
  /** Текст ссылки-действия (например, «Изменить дату рождения»). */
  actionLabel?: string;
  actionTo?: string;
  style?: CSSProperties;
}

const VARIANT_STYLES: Record<Variant, { bg: string; bd: string; fg: string; mark: string }> = {
  critical: {
    bg: 'var(--accent-tint, #f5d9d2)',
    bd: 'var(--accent-soft, #c98a7a)',
    fg: 'var(--accent-deep, #6a1f12)',
    mark: 'var(--accent-deep, #6a1f12)',
  },
  warning: {
    bg: 'var(--gold-tint, #f7ecd0)',
    bd: 'var(--gold, #c79a3a)',
    fg: 'var(--ink, #2a221a)',
    mark: 'var(--gold-deep, #8a6a1f)',
  },
};

export function CriticalNotice({
  title,
  description,
  variant = 'critical',
  actionLabel,
  actionTo,
  style,
}: CriticalNoticeProps) {
  const v = VARIANT_STYLES[variant];
  return (
    <div
      role={variant === 'critical' ? 'alert' : 'status'}
      style={{
        display: 'flex',
        gap: 12,
        padding: '12px 14px',
        background: v.bg,
        border: `1px solid ${v.bd}`,
        borderLeft: `4px solid ${v.mark}`,
        borderRadius: 2,
        fontFamily: 'var(--sans)',
        color: v.fg,
        ...style,
      }}
    >
      <div style={{ flex: 1 }}>
        <div style={{ fontWeight: 600, fontSize: 13, lineHeight: 1.4, marginBottom: description ? 4 : 0 }}>
          {variant === 'critical' ? '⚠ ' : 'ⓘ '}
          {title}
        </div>
        {description && (
          <div style={{ fontSize: 12, lineHeight: 1.5, opacity: 0.9 }}>{description}</div>
        )}
        {actionLabel && actionTo && (
          <div style={{ marginTop: 6 }}>
            <Link
              to={actionTo}
              style={{
                color: v.mark,
                fontSize: 12,
                fontWeight: 500,
                textDecoration: 'underline',
              }}
            >
              {actionLabel} →
            </Link>
          </div>
        )}
      </div>
    </div>
  );
}
