import type { CSSProperties } from 'react';
import { Icon } from './Icon';

interface ParagraphMarkProps {
  size?: number;
  variant?: 'ink' | 'on-dark';
  style?: CSSProperties;
}

/**
 * Бренд-знак — параграф в скобках `[§]` (SVG `brand-mark` из общего спрайта).
 * Раньше рисовался как `§` в сплошном квадрате; приведён к единому бренд-знаку
 * `[§]` (скобки по бокам), который используется на лендинге и в шапке.
 */
export function ParagraphMark({ size = 32, variant = 'ink', style }: ParagraphMarkProps) {
  return (
    <Icon
      name={variant === 'on-dark' ? 'brand-mark-on-dark' : 'brand-mark'}
      size={size}
      decorative
      style={{ flexShrink: 0, ...style }}
    />
  );
}
