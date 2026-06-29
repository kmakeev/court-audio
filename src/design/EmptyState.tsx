import type { CSSProperties, ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { ParagraphMark } from './ParagraphMark';
import { Icon, type IconName } from './Icon';

interface EmptyStateProps {
  /** Заголовок (serif). */
  title: string;
  /** Пояснение под заголовком. */
  description?: ReactNode;
  /** Иконка из спрайта вместо брендового §-знака. */
  icon?: IconName;
  /** Размер знака/иконки. */
  markSize?: number;
  /** CTA: текст + маршрут (router Link) ИЛИ обработчик. */
  actionLabel?: string;
  actionTo?: string;
  onAction?: () => void;
  /** Компактный режим (для секций внутри страницы, а не full-page). */
  compact?: boolean;
  style?: CSSProperties;
}

/**
 * H7 (07.11) — единый фирменный empty-state с действием.
 * Брендовый знак `§` (через ParagraphMark) либо иконка из спрайта,
 * serif-заголовок, пояснение и опциональный CTA. Заменяет разрозненные
 * ad-hoc `<p style="fontStyle:italic">`-заглушки по страницам.
 */
export function EmptyState({
  title,
  description,
  icon,
  markSize = 44,
  actionLabel,
  actionTo,
  onAction,
  compact = false,
  style,
}: EmptyStateProps) {
  const action =
    actionLabel && (actionTo || onAction) ? (
      actionTo ? (
        <Link
          to={actionTo}
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 8,
            marginTop: 4,
            background: 'var(--accent)',
            color: 'var(--paper)',
            padding: '9px 16px',
            fontSize: 13,
            fontWeight: 500,
          }}
        >
          {actionLabel}
        </Link>
      ) : (
        <button
          type="button"
          onClick={onAction}
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 8,
            marginTop: 4,
            background: 'var(--accent)',
            color: 'var(--paper)',
            padding: '9px 16px',
            fontSize: 13,
            fontWeight: 500,
          }}
        >
          {actionLabel}
        </button>
      )
    ) : null;

  return (
    <div className={`empty-state${compact ? ' empty-state--compact' : ''}`} style={style}>
      {icon ? (
        <Icon name={icon} size={markSize} decorative />
      ) : (
        <ParagraphMark size={markSize} />
      )}
      <h3 className="empty-state__title">{title}</h3>
      {description && <p className="empty-state__text">{description}</p>}
      {action}
    </div>
  );
}
