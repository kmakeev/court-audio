import { useEffect, useId, useRef, useState, type ReactNode } from 'react';

// Контурный «ⓘ» (info): незалитое кольцо + выражённая буква «i» (жирное высокое
// тело + точка), всё в текущем цвете (= цвет шрифта). Жирная «i» не даёт глифу
// читаться как «диск». Экспортируется, чтобы тот же глиф использовался и в
// баннерах `CriticalNotice` (единый значок «инфо» в программе).
export function InfoGlyph({ size }: { size: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9.4" strokeWidth="1.6" />
      <line x1="12" y1="10.6" x2="12" y2="17.6" strokeWidth="2.6" />
      <circle cx="12" cy="6.9" r="1.35" fill="currentColor" stroke="none" />
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
export function InfoTip({ text, children, label = 'Почему так?', size = 18 }: InfoTipProps) {
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
          // Полный сброс нативного вида кнопки: без него webview рисует свою
          // серую кнопку-кружок (фон/рамка/фокус-обводка) вокруг бейджа.
          appearance: 'none',
          background: 'transparent',
          border: 'none',
          outline: 'none',
          padding: 0,
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          width: size,
          height: size,
          lineHeight: 0,
          // Бледный, единый во всех подсказках (как подписи-caption). Стабильный:
          // без мигания в бордовый по щелчку — открытие подсвечивает сам поповер.
          color: 'var(--muted)',
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
