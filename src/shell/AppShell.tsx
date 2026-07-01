import type { CSSProperties } from 'react';
import { NavLink, Outlet } from 'react-router-dom';
import { Icon, type IconName } from '../design';

// Оболочка приложения в духе W2.8: тёмная шапка с бренд-знаком + боковая
// навигация. Свой компонент (а не Header из ex_system, который app-coupled) —
// стилизация только токенами PravoUI. См. .design-sync/SOURCE.md.

interface NavItem {
  to: string;
  label: string;
  icon: IconName;
}

// `end` для индекс-маршрута, чтобы «Запись» не подсвечивалась на всех путях.
const NAV: NavItem[] = [
  { to: '/', label: 'Запись', icon: 'icon-step-report' },
  { to: '/sessions', label: 'Сессии', icon: 'icon-step-case' },
  { to: '/settings', label: 'Настройки', icon: 'icon-settings' },
  { to: '/diagnostics', label: 'Диагностика', icon: 'icon-activity' },
];

const APP_TITLE = 'Аудиопротокол';
const APP_SUBTITLE = 'Аудиопротоколирование заседаний';

export function AppShell() {
  return (
    <div
      style={{
        minHeight: '100vh',
        display: 'grid',
        gridTemplateRows: 'auto 1fr',
        background: 'var(--paper)',
      }}
    >
      <header
        className="bg-dark text-on-dark"
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 16,
          padding: '16px 28px',
          borderBottom: '1px solid #000',
        }}
      >
        <Icon name="brand-mark-on-dark" size={36} decorative />
        <div style={{ display: 'flex', flexDirection: 'column' }}>
          <span
            style={{
              fontFamily: 'var(--serif)',
              fontSize: 20,
              fontWeight: 500,
              letterSpacing: '-0.01em',
              color: 'var(--on-dark)',
            }}
          >
            {APP_TITLE}
          </span>
          <span
            className="text-on-dark-soft"
            style={{ fontSize: 12, letterSpacing: '0.02em' }}
          >
            {APP_SUBTITLE}
          </span>
        </div>
      </header>

      <div style={{ display: 'grid', gridTemplateColumns: '232px 1fr' }}>
        <nav
          aria-label="Основная навигация"
          style={{
            borderRight: '1px solid var(--hairline)',
            background: 'var(--paper-soft)',
            padding: '20px 12px',
            display: 'flex',
            flexDirection: 'column',
            gap: 4,
          }}
        >
          {NAV.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.to === '/'}
              style={({ isActive }) => navLinkStyle(isActive)}
            >
              <Icon name={item.icon} size={18} decorative />
              <span>{item.label}</span>
            </NavLink>
          ))}
        </nav>

        <main style={{ padding: '28px 32px', minWidth: 0 }}>
          <Outlet />
        </main>
      </div>
    </div>
  );
}

function navLinkStyle(isActive: boolean): CSSProperties {
  return {
    display: 'flex',
    alignItems: 'center',
    gap: 12,
    padding: '10px 14px',
    fontFamily: 'var(--sans)',
    fontSize: 14,
    fontWeight: 500,
    textDecoration: 'none',
    color: isActive ? 'var(--accent-deep)' : 'var(--ink-soft)',
    background: isActive ? 'var(--accent-tint)' : 'transparent',
    borderLeft: isActive
      ? '3px solid var(--accent)'
      : '3px solid transparent',
  };
}
