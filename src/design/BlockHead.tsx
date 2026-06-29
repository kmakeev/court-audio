import type { ReactNode } from 'react';
import { Icon, type IconName } from './Icon';

interface BlockHeadProps {
  numeral: string; // "A", "B", "01"
  title: ReactNode;
  hint?: ReactNode;
  /**
   * Раунд 06_4_b (F3, 2026-06-02): опциональная иконка нормы УК справа от
   * заголовка/`hint` — сигнализирует тип нормы (статья / часть / правило /
   * смягчающее / отягчающее / рецидив / категория). Когда `iconName` не
   * передан — старый layout без изменений.
   */
  iconName?: IconName;
  /** Подсказка для иконки (a11y). Если не указано — иконка decorative. */
  iconTitle?: string;
}

export function BlockHead({ numeral, title, hint, iconName, iconTitle }: BlockHeadProps) {
  return (
    <div style={{ marginBottom: 10 }}>
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 14, flexWrap: 'wrap' }}>
        <span
          className="num"
          style={{
            fontSize: 11,
            color: 'var(--accent)',
            letterSpacing: '0.1em',
            fontWeight: 500,
            width: 18,
            flexShrink: 0,
          }}
        >
          {numeral}
        </span>
        <h2
          style={{
            margin: 0,
            fontFamily: 'var(--serif)',
            fontWeight: 500,
            fontSize: 22,
            color: 'var(--ink)',
            letterSpacing: '-0.01em',
            // 2026-05-26: убрали flexShrink:0 — длинные заголовки не
            // переполняют контейнер; при необходимости переносятся.
            flexShrink: 1,
            minWidth: 0,
          }}
        >
          {title}
        </h2>
        {/* Double-line rule: border-top + border-bottom with gap */}
        <span
          style={{
            flex: 1,
            borderTop: '1px solid var(--hairline)',
            borderBottom: '1px solid var(--hairline)',
            height: 4,
            marginLeft: 8,
            marginBottom: 4,
            alignSelf: 'flex-end',
          }}
          aria-hidden="true"
        />
        {iconName && (
          <Icon
            name={iconName}
            size={16}
            title={iconTitle}
            decorative={!iconTitle}
            style={{
              color: 'var(--ink-soft)',
              alignSelf: 'flex-end',
              marginBottom: 4,
              marginLeft: 8,
            }}
          />
        )}
      </div>
      {hint && (
        <div
          style={{
            fontSize: 12,
            color: 'var(--muted)',
            fontFamily: 'var(--sans)',
            marginTop: 2,
            paddingLeft: 32,
          }}
        >
          {hint}
        </div>
      )}
    </div>
  );
}
