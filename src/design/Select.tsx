/**
 * Кастомный select в стиле сайта (а не нативный OS-dropdown).
 * Создан 2026-05-19 по замечанию заказчика: до этого использовался
 * `<select>`, чей дроп открывался в OS-стиле.
 *
 * Поддерживает:
 *  - кнопка-trigger в стиле inputs дизайн-системы;
 *  - выпадающий список с keyboard navigation (↑/↓/Enter/Esc);
 *  - placeholder, disabled, error-state (через `aria-invalid`);
 *  - portal не используется — `<ul>` рендерится absolute под триггером
 *    (для модалок это удобнее: не пересекается с backdrop-логикой).
 */

import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
} from 'react';
import { createPortal } from 'react-dom';
import { CONTROL_HEIGHT } from './patterns';

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export interface SelectProps {
  options: SelectOption[];
  value: string;
  onChange: (next: string) => void;
  placeholder?: string;
  disabled?: boolean;
  name?: string;
  id?: string;
  /** Помечать триггер как ошибочный (.is-error-like рамка). */
  invalid?: boolean;
  /** Дополнительный inline style на триггере. */
  triggerStyle?: CSSProperties;
  /** ARIA-label для триггера, если рядом нет `<label>`. */
  ariaLabel?: string;
}

const ARROW_SVG = (
  <svg
    aria-hidden="true"
    width="10"
    height="6"
    viewBox="0 0 10 6"
    style={{ flexShrink: 0 }}
  >
    <path fill="none" stroke="currentColor" strokeWidth="1.4" d="M1 1l4 4 4-4" />
  </svg>
);

export function Select({
  options,
  value,
  onChange,
  placeholder = '— не выбрано —',
  disabled,
  name,
  id,
  invalid,
  triggerStyle,
  ariaLabel,
}: SelectProps) {
  const reactId = useId();
  const autoId = id ?? `sel-${reactId}`;
  const listboxId = `${autoId}-list`;
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  const [open, setOpen] = useState(false);
  const [hl, setHl] = useState(-1);
  const [coords, setCoords] = useState<{ top: number; left: number; width: number } | null>(null);

  const currentIdx = options.findIndex((o) => o.value === value);
  const current = currentIdx >= 0 ? options[currentIdx] : null;

  const computeCoords = useCallback(() => {
    const el = triggerRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setCoords({ top: r.bottom + window.scrollY, left: r.left + window.scrollX, width: r.width });
  }, []);

  useLayoutEffect(() => {
    if (!open) return;
    computeCoords();
    setHl(currentIdx >= 0 ? currentIdx : 0);
  }, [open, computeCoords, currentIdx]);

  useEffect(() => {
    if (!open) return;
    const onResize = () => computeCoords();
    const onDocClick = (e: MouseEvent) => {
      const t = e.target as Node;
      if (
        triggerRef.current && !triggerRef.current.contains(t) &&
        listRef.current && !listRef.current.contains(t)
      ) {
        setOpen(false);
      }
    };
    const onScroll = () => computeCoords();
    window.addEventListener('resize', onResize);
    window.addEventListener('scroll', onScroll, true);
    document.addEventListener('mousedown', onDocClick);
    return () => {
      window.removeEventListener('resize', onResize);
      window.removeEventListener('scroll', onScroll, true);
      document.removeEventListener('mousedown', onDocClick);
    };
  }, [open, computeCoords]);

  const choose = (idx: number) => {
    const opt = options[idx];
    if (!opt || opt.disabled) return;
    onChange(opt.value);
    setOpen(false);
    triggerRef.current?.focus();
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLButtonElement | HTMLUListElement>) => {
    if (disabled) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (!open) {
        setOpen(true);
      } else {
        setHl((p) => {
          let next = p + 1;
          while (next < options.length && options[next]?.disabled) next++;
          return next < options.length ? next : p;
        });
      }
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (!open) setOpen(true);
      else {
        setHl((p) => {
          let next = p - 1;
          while (next >= 0 && options[next]?.disabled) next--;
          return next >= 0 ? next : p;
        });
      }
      return;
    }
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      if (!open) setOpen(true);
      else if (hl >= 0) choose(hl);
      return;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      setOpen(false);
      triggerRef.current?.focus();
      return;
    }
    if (e.key === 'Tab') {
      setOpen(false);
    }
  };

  const triggerCss: CSSProperties = {
    appearance: 'none',
    width: '100%',
    background: disabled ? 'var(--paper-soft)' : 'var(--paper-elev)',
    border: invalid ? '1px solid var(--accent-deep)' : '1px solid var(--hairline)',
    boxShadow: invalid ? '0 0 0 1px var(--accent-deep)' : undefined,
    padding: '10px 36px 10px 12px',
    fontSize: 14,
    color: current ? 'var(--ink)' : 'var(--muted)',
    fontFamily: 'inherit',
    cursor: disabled ? 'not-allowed' : 'pointer',
    textAlign: 'left',
    position: 'relative',
    outline: 'none',
    // Общий токен высоты контролов: `Select` совпадает по высоте с `Field`
    // в одном ряду фильтров/формы (R-012).
    minHeight: CONTROL_HEIGHT,
    display: 'flex',
    alignItems: 'center',
    ...triggerStyle,
  };

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        id={autoId}
        name={name}
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={listboxId}
        aria-disabled={disabled}
        aria-invalid={invalid}
        aria-label={ariaLabel}
        disabled={disabled}
        onClick={() => !disabled && setOpen((v) => !v)}
        onKeyDown={onKeyDown}
        style={triggerCss}
      >
        <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {current ? current.label : (placeholder || ' ')}
        </span>
        <span
          aria-hidden="true"
          style={{
            position: 'absolute',
            right: 12,
            top: '50%',
            transform: `translateY(-50%) rotate(${open ? 180 : 0}deg)`,
            color: 'var(--muted)',
            transition: 'transform .12s ease',
            display: 'inline-flex',
          }}
        >
          {ARROW_SVG}
        </span>
      </button>

      {open && coords && createPortal(
        <ul
          ref={listRef}
          id={listboxId}
          role="listbox"
          tabIndex={-1}
          onKeyDown={onKeyDown}
          style={{
            position: 'absolute',
            top: coords.top + 4,
            left: coords.left,
            width: coords.width,
            zIndex: 200,
            margin: 0,
            padding: '4px 0',
            listStyle: 'none',
            background: 'var(--paper-elev)',
            border: '1px solid var(--hairline)',
            boxShadow: '0 10px 28px rgba(0, 0, 0, 0.14)',
            maxHeight: 280,
            overflowY: 'auto',
            fontSize: 14,
            color: 'var(--ink)',
            fontFamily: 'inherit',
          }}
        >
          {options.length === 0 && (
            <li
              role="option"
              aria-selected={false}
              aria-disabled
              style={{
                padding: '8px 12px',
                color: 'var(--muted)',
                fontStyle: 'italic',
              }}
            >
              {placeholder ?? 'Нет значений'}
            </li>
          )}
          {options.map((opt, i) => {
            const selected = opt.value === value;
            const highlighted = i === hl;
            return (
              // Клавиатура обрабатывается на родительском role="listbox"
              // (roving highlight + Enter → choose); onClick — мышиный путь.
              // eslint-disable-next-line jsx-a11y/click-events-have-key-events
              <li
                key={opt.value || `__${i}`}
                role="option"
                aria-selected={selected}
                aria-disabled={opt.disabled}
                onMouseEnter={() => setHl(i)}
                onClick={() => !opt.disabled && choose(i)}
                style={{
                  padding: '8px 12px',
                  cursor: opt.disabled ? 'not-allowed' : 'pointer',
                  background: highlighted
                    ? 'var(--paper-strong)'
                    : selected
                      ? 'var(--paper-soft)'
                      : 'transparent',
                  color: opt.disabled
                    ? 'var(--muted)'
                    : selected
                      ? 'var(--ink)'
                      : 'var(--body)',
                  fontWeight: selected ? 500 : 400,
                  display: 'flex',
                  alignItems: 'center',
                  gap: 8,
                }}
              >
                {selected && (
                  <span
                    aria-hidden="true"
                    style={{ color: 'var(--accent)', fontFamily: 'var(--mono)', fontSize: 12 }}
                  >
                    ✓
                  </span>
                )}
                <span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {opt.label}
                </span>
              </li>
            );
          })}
        </ul>,
        document.body,
      )}
    </>
  );
}
