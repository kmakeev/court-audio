import type { CSSProperties } from 'react';

/**
 * SVG-иконка из общего sprite'а `public/icons.svg`.
 * Создан 2026-05-28 (раунд 06_4 / V.1) на основе brand-bundle от Claude
 * Design (`development/design/prompts/screens/UI/W2_brand_icons/`).
 *
 * Использование:
 * ```tsx
 * <Icon name="brand-mark" size={32} />
 * <Icon name="icon-step-case" size={20} />
 * <Icon name="icon-add" size={14} title="Добавить" />
 * ```
 *
 * Через `<use href="/icons.svg#id">` браузер подтянет sprite один раз
 * и переиспользует на всех экранах (HTTP cache + DOM-reuse).
 */

export type IconName =
  // Brand
  | 'brand-mark'
  | 'brand-mark-on-dark'
  | 'brand-mark-mono'
  | 'brand-mark-filled'
  // Stepper steps (по одной на шаг мастера)
  | 'icon-step-case'
  | 'icon-step-defendants'
  | 'icon-step-prior'
  | 'icon-step-episodes'
  | 'icon-step-aggregate'
  | 'icon-step-report'
  | 'icon-step-handoff'
  // Виды наказания (ст. 44 УК РФ)
  | 'icon-pun-fine'
  | 'icon-pun-disqualify'
  | 'icon-pun-mandatory'
  | 'icon-pun-corrective'
  | 'icon-pun-restriction'
  | 'icon-pun-forced'
  | 'icon-pun-arrest'
  | 'icon-pun-prison'
  | 'icon-pun-life'
  | 'icon-pun-death'
  // Действия / навигация
  | 'icon-add'
  | 'icon-close'
  | 'icon-edit'
  | 'icon-forward'
  | 'icon-download'
  | 'icon-upload'
  | 'icon-retry'
  | 'icon-external'
  | 'icon-check'
  | 'icon-search'
  | 'icon-calculate'
  | 'icon-bell'
  | 'icon-feedback'
  // Типы норм УК
  | 'icon-norm-article'
  | 'icon-norm-part'
  | 'icon-norm-point'
  | 'icon-norm-rule'
  | 'icon-norm-aggrav'
  | 'icon-norm-mitig'
  | 'icon-norm-recid'
  | 'icon-norm-category'
  // AI (раунд 06_3_A1)
  | 'icon-ai';

interface IconProps {
  name: IconName;
  /** Сторона квадрата в px. Default — 16. */
  size?: number;
  /** Подсказка при hover (а также а11y-метка). */
  title?: string;
  className?: string;
  style?: CSSProperties;
  /** Цвет для моно-иконок (используют `currentColor`). */
  color?: string;
  /** Маркер «декоративная» — не озвучивается экранным читателем. */
  decorative?: boolean;
}

export function Icon({
  name,
  size = 16,
  title,
  className,
  style,
  color,
  decorative,
}: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      role={decorative ? 'presentation' : 'img'}
      aria-hidden={decorative || undefined}
      aria-label={!decorative && title ? title : undefined}
      focusable="false"
      className={className}
      style={{
        // Tailwind preflight делает `svg { display: block }` — это
        // рвёт inline-flow когда Icon вложен в `<span>` рядом с текстом
        // (см. SubjectCard, EpisodeSummary, EpisodesListPage). Явный
        // `inline-block` восстанавливает inline-поведение; в flex-row
        // он остаётся flex-item'ом (display не влияет на flex children).
        display: 'inline-block',
        flexShrink: 0,
        color,
        ...style,
      }}
    >
      {title && !decorative && <title>{title}</title>}
      <use href={`/icons.svg#${name}`} />
    </svg>
  );
}
