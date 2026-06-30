import type { InputHTMLAttributes, ReactNode } from 'react';
import { forwardRef } from 'react';

interface FieldProps extends InputHTMLAttributes<HTMLInputElement> {
  label: string;
  error?: string;
  trailing?: ReactNode;
}

export const Field = forwardRef<HTMLInputElement, FieldProps>(function Field(
  { label, error, trailing, id, ...rest },
  ref,
) {
  const inputId = id ?? `field-${rest.name ?? Math.random().toString(36).slice(2)}`;
  return (
    <label htmlFor={inputId} style={{ display: 'block' }}>
      <span
        style={{
          display: 'block',
          fontSize: 11,
          textTransform: 'uppercase',
          letterSpacing: '0.14em',
          color: 'var(--muted)',
          marginBottom: 8,
          fontWeight: 500,
        }}
      >
        {label}
      </span>
      <div
        style={{
          position: 'relative',
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
            border: 0,
            background: 'transparent',
            padding: trailing ? '12px 44px 12px 14px' : '12px 14px',
            fontFamily: 'var(--sans)',
            fontSize: 14,
            color: 'var(--ink)',
            outline: 'none',
            lineHeight: 1.4,
          }}
        />
        {trailing}
      </div>
      {error && (
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
      )}
    </label>
  );
});
