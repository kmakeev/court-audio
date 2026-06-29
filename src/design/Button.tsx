import type { ButtonHTMLAttributes, CSSProperties, ReactNode } from 'react';

type Variant = 'primary' | 'secondary' | 'mini' | 'link';

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  loading?: boolean;
  leftIcon?: ReactNode;
  rightIcon?: ReactNode;
}

const baseStyles: Record<Variant, CSSProperties> = {
  primary: {
    background: 'var(--accent)',
    color: 'var(--paper)',
    height: 44,
    padding: '0 20px',
    fontWeight: 500,
    fontSize: 14,
    letterSpacing: '0.02em',
  },
  secondary: {
    background: 'transparent',
    color: 'var(--on-dark)',
    height: 38,
    padding: '0 16px',
    border: '1px solid #3a352c',
    fontSize: 13,
    fontWeight: 500,
  },
  mini: {
    background: 'var(--paper-elev)',
    color: 'var(--ink-soft)',
    height: 24,
    padding: '0 10px',
    border: '1px solid var(--hairline)',
    fontSize: 11,
    fontWeight: 500,
  },
  link: {
    background: 'transparent',
    color: 'var(--accent)',
    fontSize: 13,
    fontWeight: 500,
    textDecoration: 'underline',
    textUnderlineOffset: 3,
    padding: 0,
    height: 'auto',
  },
};

export function Button({
  variant = 'primary',
  loading = false,
  leftIcon,
  rightIcon,
  disabled,
  children,
  style,
  ...rest
}: ButtonProps) {
  const isDisabled = disabled || loading;
  return (
    <button
      type="button"
      disabled={isDisabled}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 10,
        cursor: isDisabled ? 'default' : 'pointer',
        opacity: isDisabled ? 0.55 : 1,
        transition: 'background 120ms ease, opacity 120ms ease',
        ...baseStyles[variant],
        ...style,
      }}
      {...rest}
    >
      {leftIcon}
      <span>{children}</span>
      {rightIcon}
    </button>
  );
}
