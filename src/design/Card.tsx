import type { CSSProperties, HTMLAttributes } from 'react';

type CardVariant = 'plain' | 'accent';

interface CardProps extends HTMLAttributes<HTMLDivElement> {
  variant?: CardVariant;
}

export function Card({ variant = 'plain', style, children, ...rest }: CardProps) {
  const composed: CSSProperties = {
    background: 'var(--paper-elev)',
    border: '1px solid var(--hairline)',
    ...(variant === 'accent' ? { borderLeft: '4px solid var(--accent)' } : {}),
    padding: '20px 24px',
    ...style,
  };
  return (
    <div style={composed} {...rest}>
      {children}
    </div>
  );
}
