import type { CSSProperties } from 'react';
import { NavLink, Outlet, useNavigate } from 'react-router-dom';
import { Icon, type IconName } from '../design';
import { useAuth } from '../lib/auth-context';
import { authLogout } from '../lib/core';

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
  { to: '/administration', label: 'Администрирование', icon: 'icon-norm-rule' },
  { to: '/diagnostics', label: 'Диагностика', icon: 'icon-activity' },
];

const APP_TITLE = 'Аудиопротокол';
const APP_SUBTITLE = 'Аудиопротоколирование заседаний';

export function AppShell() {
  const navigate = useNavigate();
  const { status, refresh } = useAuth();
  const operator = status?.operator ?? null;

  async function onLogout() {
    try {
      await authLogout();
    } finally {
      refresh();
      navigate('/login', { replace: true });
    }
  }

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

        {operator && (
          <div
            style={{
              marginLeft: 'auto',
              display: 'flex',
              alignItems: 'center',
              gap: 16,
            }}
          >
            <ConnectionBadge online={status?.online ?? false} />
            <div style={{ display: 'flex', flexDirection: 'column', textAlign: 'right' }}>
              <span style={{ fontSize: 14, fontWeight: 500, color: 'var(--on-dark)' }}>
                {operator.full_name || `Оператор #${operator.operator_id}`}
              </span>
              <span className="text-on-dark-soft" style={{ fontSize: 12 }}>
                {roleLabel(operator.role)}
              </span>
            </div>
            <button
              type="button"
              onClick={() => void onLogout()}
              style={{
                background: 'transparent',
                border: '1px solid var(--on-dark-soft, #ffffff55)',
                color: 'var(--on-dark)',
                fontFamily: 'var(--sans)',
                fontSize: 13,
                padding: '6px 12px',
                cursor: 'pointer',
              }}
            >
              Сменить оператора
            </button>
          </div>
        )}
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

// Индикатор связи с сервером: онлайн-сессия vs оффлайн-разблокировка по кэшу.
function ConnectionBadge({ online }: { online: boolean }) {
  return (
    <span
      title={online ? 'Связь с сервером есть' : 'Оффлайн-режим (кэшированная сессия)'}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 6,
        fontSize: 12,
        color: 'var(--on-dark-soft, #ffffffaa)',
      }}
    >
      <span
        aria-hidden
        style={{
          width: 8,
          height: 8,
          borderRadius: '50%',
          background: online ? '#3fb950' : '#d29922',
        }}
      />
      {online ? 'Онлайн' : 'Оффлайн'}
    </span>
  );
}

// Человекочитаемая метка роли оператора (роли задаёт ex_system).
function roleLabel(role: string): string {
  const map: Record<string, string> = {
    admin: 'Администратор',
    judge: 'Судья',
    assistant: 'Помощник судьи',
    analyst: 'Аналитик',
    user: 'Пользователь',
  };
  return map[role] ?? role;
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
