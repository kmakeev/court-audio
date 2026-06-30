import type { CSSProperties } from 'react';

type SkeletonVariant = 'line' | 'block' | 'circle';

interface SkeletonProps {
  /** Ширина плашки (число → px). По умолчанию 100%. */
  width?: number | string;
  /** Высота плашки (число → px). По умолчанию зависит от variant. */
  height?: number | string;
  /** border-radius (число → px). По умолчанию 3 (для circle игнорируется). */
  radius?: number | string;
  /** line — тонкая строка текста; block — прямоугольный блок; circle — круг. */
  variant?: SkeletonVariant;
  /** Повторить N одинаковых плашек (вертикальный стек с gap). */
  count?: number;
  className?: string;
  style?: CSSProperties;
}

/**
 * 07.13 (H8.1) — skeleton-плейсхолдер на время первичной загрузки данных.
 *
 * Не путать с `EmptyState` (для «данных нет»): Skeleton — для «данные грузятся».
 * Показывать только при первичной загрузке (`isLoading && !data`); фоновые
 * `isFetching` поверх отрисованных данных skeleton не показывают.
 *
 * A11y: контейнер несёт `aria-busy="true"`, сами плашки — `aria-hidden`
 * (скринридер не зачитывает «пустые» строки). Shimmer выключается при
 * `prefers-reduced-motion: reduce` (см. styles/skeleton.css).
 */
export function Skeleton({
  width,
  height,
  radius = 3,
  variant = 'line',
  count = 1,
  className,
  style,
}: SkeletonProps) {
  const defaultHeight = variant === 'line' ? 13 : variant === 'circle' ? 32 : 56;
  const h = height ?? defaultHeight;
  const w = variant === 'circle' ? (width ?? h) : (width ?? '100%');
  const br = variant === 'circle' ? '50%' : radius;

  const plate = (key?: number) => (
    <span
      key={key}
      aria-hidden="true"
      className={`skeleton${className ? ` ${className}` : ''}`}
      style={{
        display: 'block',
        width: typeof w === 'number' ? `${w}px` : w,
        height: typeof h === 'number' ? `${h}px` : h,
        borderRadius: typeof br === 'number' ? `${br}px` : br,
        flexShrink: 0,
        ...style,
      }}
    />
  );

  return (
    <span aria-busy="true" style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      {count > 1 ? Array.from({ length: count }, (_, i) => plate(i)) : plate()}
    </span>
  );
}
