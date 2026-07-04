import type { InputHTMLAttributes, ReactNode } from 'react';
import { forwardRef } from 'react';
import { CONTROL_HEIGHT } from './patterns';

interface FieldProps extends InputHTMLAttributes<HTMLInputElement> {
  label: string;
  error?: string;
  trailing?: ReactNode;
  /** Необязательная подсказка «ⓘ» рядом с подписью (обычно `<InfoTip/>`). */
  tip?: ReactNode;
}

const captionStyle = {
  fontSize: 11,
  textTransform: 'uppercase' as const,
  letterSpacing: '0.14em',
  color: 'var(--muted)',
  fontWeight: 500,
};

export const Field = forwardRef<HTMLInputElement, FieldProps>(function Field(
  { label, error, trailing, tip, id, ...rest },
  ref,
) {
  const inputId = id ?? `field-${rest.name ?? Math.random().toString(36).slice(2)}`;
  const box = (
    <div
      style={{
        position: 'relative',
        // Высоту из общего токена контролов держит бордюрная обёртка (а не input),
        // чтобы её собственная рамка входила в высоту (box-sizing: border-box) и
        // `Field` совпадал по высоте с `Select` в одном ряду (R-012). Иначе рамка
        // обёртки добавляла бы ~2 px поверх высоты input'а.
        height: CONTROL_HEIGHT,
        background: 'var(--paper-elev)',
        border: error ? '1px solid var(--accent)' : '1px solid var(--hairline)',
        transition: 'border-color 120ms ease',
      }}
    >
      <input
        id={inputId}
        ref={ref}
        {...rest}
        style={{
          width: '100%',
          height: '100%',
          border: 0,
          background: 'transparent',
          padding: trailing ? '0 44px 0 14px' : '0 14px',
          fontFamily: 'var(--sans)',
          fontSize: 14,
          color: 'var(--ink)',
          outline: 'none',
          lineHeight: 1.4,
          boxSizing: 'border-box',
        }}
      />
      {trailing}
    </div>
  );
  const errNode = error && (
    <div
      role="alert"
      style={{
        marginTop: 8,
        fontFamily: 'var(--serif)',
        fontStyle: 'italic',
        fontSize: 12,
        color: 'var(--accent)',
        lineHeight: 1.4,
      }}
    >
      {error}
    </div>
  );

  // С подсказкой: «ⓘ» вынесен из стилизованной подписи (иначе унаследовал бы
  // `text-transform`/`letter-spacing`), а сам триггер-кнопка — вне `<label>`,
  // чтобы клик по «ⓘ» не фокусировал поле. Ассоциация подписи с input — по
  // `htmlFor`/`id` (как и без подсказки).
  if (tip) {
    return (
      <div style={{ display: 'block' }}>
        <span style={{ display: 'flex', alignItems: 'center', gap: 4, marginBottom: 8 }}>
          <label htmlFor={inputId} style={captionStyle}>
            {label}
          </label>
          {tip}
        </span>
        {box}
        {errNode}
      </div>
    );
  }

  return (
    <label htmlFor={inputId} style={{ display: 'block' }}>
      <span style={{ display: 'block', marginBottom: 8, ...captionStyle }}>{label}</span>
      {box}
      {errNode}
    </label>
  );
});
