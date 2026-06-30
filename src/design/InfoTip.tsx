import { useEffect, useId, useRef, useState, type ReactNode } from 'react';

// Кружок-«ⓘ» (info). Отдельный значок, чтобы не переиспользовать иконку нормы.
function InfoGlyph({ size }: { size: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" />
      <line x1="12" y1="11" x2="12" y2="16.5" />
      <circle cx="12" cy="7.5" r="0.9" fill="currentColor" stroke="none" />
    </svg>
  );
}

interface InfoTipProps {
  /** Текст-пояснение (готовый reason из applied_rules / DecisionStep). */
  text?: ReactNode;
  children?: ReactNode;
  /** Доступное имя кнопки (по умолчанию «Почему так?»). */
  label?: string;
  /** Размер иконки-триггера. */
  size?: number;
}

/**
 * H7 (07.11) — точечный тултип «почему так?». Раскрывает УЖЕ вычисленное
 * обоснование (reason), ничего не пересчитывает. Доступен с клавиатуры:
 * кнопка `aria-expanded` + `aria-describedby`, Esc закрывает, клик вне —
 * закрывает. Стиль — дизайн-система (`.infotip-pop`).
 */
export function InfoTip({ text, children, label = 'Почему так?', size = 14 }: InfoTipProps) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const wrapRef = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') setOpen(false);
    }
    function onClick(e: MouseEvent) {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onClick);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onClick);
    };
  }, [open]);

  const content = children ?? text;

  return (
    <span ref={wrapRef} style={{ position: 'relative', display: 'inline-flex' }}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-describedby={open ? id : undefined}
        aria-label={label}
        title={label}
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          width: size + 6,
          height: size + 6,
          borderRadius: '50%',
          color: open ? 'var(--accent)' : 'var(--muted)',
          cursor: 'pointer',
        }}
      >
        <InfoGlyph size={size} />
      </button>
      {open && content && (
        <span
          id={id}
          role="tooltip"
          className="infotip-pop"
          style={{ top: size + 12, left: 0 }}
        >
          {content}
        </span>
      )}
    </span>
  );
}
