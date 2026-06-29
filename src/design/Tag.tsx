import type { HTMLAttributes } from 'react';

type TagTone = 'default' | 'accent' | 'gold' | 'green';

interface TagProps extends HTMLAttributes<HTMLSpanElement> {
  tone?: TagTone;
}

const toneMap: Record<TagTone, { bg: string; fg: string; bd: string }> = {
  default: { bg: 'var(--paper-strong)', fg: 'var(--ink-soft)', bd: 'var(--hairline)' },
  accent: { bg: 'var(--accent-tint)', fg: 'var(--accent-deep)', bd: 'var(--accent-soft)' },
  gold: { bg: 'var(--gold-tint)', fg: 'var(--gold)', bd: 'var(--gold)' },
  green: { bg: '#dde6ce', fg: 'var(--green)', bd: 'var(--green)' },
};

export function Tag({ tone = 'default', style, ...rest }: TagProps) {
  const t = toneMap[tone];
  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 6,
        padding: '2px 8px',
        background: t.bg,
        color: t.fg,
        border: `1px solid ${t.bd}`,
        borderRadius: 2,
        fontFamily: 'var(--sans)',
        fontSize: 12,
        fontWeight: 500,
        lineHeight: 1.4,
        ...style,
      }}
      {...rest}
    />
  );
}
