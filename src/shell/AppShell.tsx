import { useCallback, useEffect, useState, type CSSProperties } from 'react';
import { NavLink, Outlet, useNavigate } from 'react-router-dom';
import { Icon, Tag, type IconName } from '../design';
import { useAuth } from '../lib/auth-context';
import { authLogout, onSettingsSaved, openCompactOverlay } from '../lib/core';
import { getSettings } from '../lib/settings';
import { formatClock } from '../lib/format';
import {
  isSessionActive,
  recordingStatusLabel,
  recordingStatusTone,
  RecordingStatusProvider,
  useRecordingStatus,
} from '../lib/recording-status';
import { HallMode } from '../components/HallMode';
import { ConfirmDialog } from './ConfirmDialog';
import { useCloseGuard } from './useCloseGuard';

// Оболочка приложения в духе W2.8: тёмная шапка с бренд-знаком + боковая
// навигация. Свой компонент (а не Header из ex_system, который app-coupled) —
// стилизация только токенами PravoUI. Адаптивность (этап 10.5): раскладка/
// брейкпоинты — в globals.css (`.app-shell*`); здесь только состояние навигации
// и глобальный индикатор записи (виден с любого экрана).

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

/** Доступность режимов интерфейса (реестр `ui.*`). */
interface UiFlags {
  hallMode: boolean;
  compactOverlay: boolean;
}

export function AppShell() {
  return (
    <RecordingStatusProvider>
      <AppShellInner />
    </RecordingStatusProvider>
  );
}

function AppShellInner() {
  const navigate = useNavigate();
  const { status, refresh } = useAuth();
  const operator = status?.operator ?? null;

  // Защита закрытия окна с идущей записью (этап 10.6): подтверждение перед выходом.
  const { state: recState } = useRecordingStatus();
  const closeGuard = useCloseGuard(isSessionActive(recState));

  // Состояние адаптивной навигации (бургер на узких окнах) и режима зала.
  const [navOpen, setNavOpen] = useState(false);
  const [hallOpen, setHallOpen] = useState(false);
  const [ui, setUi] = useState<UiFlags>({ hallMode: true, compactOverlay: false });

  // Флаги режимов интерфейса читаем из реестра `ui.*`. Перечитываем не только при
  // монтировании, но и по событию `settings_saved` — иначе включённый в
  // «Администрировании» оверлей/режим зала не появился бы до перезапуска.
  const loadUiFlags = useCallback(() => {
    getSettings()
      .then((s) =>
        setUi({
          hallMode: s.ui.hall_mode.enabled,
          compactOverlay: s.ui.compact_overlay.enabled,
        }),
      )
      .catch(() => {});
  }, []);

  useEffect(() => {
    loadUiFlags();
    let unlisten: (() => void) | undefined;
    let active = true;
    onSettingsSaved(loadUiFlags).then((u) => {
      if (active) unlisten = u;
      else u();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [loadUiFlags]);

  // Esc закрывает выехавшую навигацию и режим зала (клавиатурная доступность).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setNavOpen(false);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  async function onLogout() {
    try {
      await authLogout();
    } finally {
      refresh();
      navigate('/login', { replace: true });
    }
  }

  return (
    <div className="app-shell">
      <header className="app-shell__header bg-dark text-on-dark">
        {operator && (
          <button
            type="button"
            className="app-shell__burger"
            aria-label={navOpen ? 'Скрыть навигацию' : 'Показать навигацию'}
            aria-expanded={navOpen}
            onClick={() => setNavOpen((v) => !v)}
          >
            <span aria-hidden style={{ fontSize: 20, lineHeight: 1 }}>
              ☰
            </span>
          </button>
        )}

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
              flexWrap: 'wrap',
              justifyContent: 'flex-end',
            }}
          >
            <HeaderRecordingBadge onOpen={() => navigate('/')} />
            <HeaderModeButtons
              ui={ui}
              onHall={() => setHallOpen(true)}
            />
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

      <div className="app-shell__body">
        {/* Затемнение под выехавшей навигацией (узкие окна) — клик закрывает. */}
        {navOpen && (
          <button
            type="button"
            className="app-shell__scrim"
            aria-label="Закрыть навигацию"
            onClick={() => setNavOpen(false)}
          />
        )}
        <nav
          aria-label="Основная навигация"
          className={`app-shell__nav${navOpen ? ' app-shell__nav--open' : ''}`}
        >
          {NAV.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.to === '/'}
              title={item.label}
              className="app-shell__nav-item"
              style={({ isActive }) => navLinkStyle(isActive)}
              onClick={() => setNavOpen(false)}
            >
              <Icon name={item.icon} size={18} decorative />
              <span className="app-shell__nav-label">{item.label}</span>
            </NavLink>
          ))}
        </nav>

        <main className="app-shell__main">
          <Outlet />
        </main>
      </div>

      {ui.hallMode && hallOpen && <HallMode onClose={() => setHallOpen(false)} />}

      {/* Защита закрытия окна с идущей записью (этап 10.6): подтверждение выхода. */}
      <ConfirmDialog
        open={closeGuard.pending}
        title="Закрыть приложение во время записи?"
        description="Идёт запись заседания. Рекомендуется сначала остановить её на экране «Запись». Закрытие приложения не прерывает уже сохранённые сегменты, но новая запись прекратится."
        confirmLabel="Всё равно закрыть"
        cancelLabel="Остаться"
        tone="danger"
        onConfirm={closeGuard.confirmClose}
        onCancel={closeGuard.cancelClose}
      />
    </div>
  );
}

// Глобальный индикатор записи в шапке: состояние + хронометр, виден при активной
// сессии из любого экрана (`role="status"` — озвучивается скринридером). Клик
// уводит на экран «Запись».
function HeaderRecordingBadge({ onOpen }: { onOpen: () => void }) {
  const { state, elapsedSec } = useRecordingStatus();
  if (!isSessionActive(state)) return null;
  return (
    <button
      type="button"
      onClick={onOpen}
      title="К экрану записи"
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 8,
        background: 'transparent',
        border: 0,
        padding: 0,
        cursor: 'pointer',
      }}
    >
      <Tag tone={recordingStatusTone(state)} role="status" aria-live="polite">
        {state === 'recording' && <RecordingDot />}
        {recordingStatusLabel(state)}
      </Tag>
      <span
        className="num"
        aria-label="Хронометраж записи"
        style={{
          fontSize: 15,
          fontVariantNumeric: 'tabular-nums',
          color: 'var(--on-dark)',
        }}
      >
        {formatClock(elapsedSec)}
      </span>
    </button>
  );
}

// Кнопки режимов интерфейса (режим зала / окно поверх всех окон). Гейт — реестр
// `ui.*`: недоступный режим не показывает кнопку.
function HeaderModeButtons({ ui, onHall }: { ui: UiFlags; onHall: () => void }) {
  const [overlayError, setOverlayError] = useState(false);
  if (!ui.hallMode && !ui.compactOverlay) return null;
  return (
    <div style={{ display: 'flex', gap: 8 }}>
      {ui.hallMode && (
        <button type="button" onClick={onHall} style={headerModeBtnStyle} title="Крупный статус записи">
          Режим зала
        </button>
      )}
      {ui.compactOverlay && (
        <button
          type="button"
          onClick={() => {
            openCompactOverlay().catch(() => setOverlayError(true));
          }}
          style={headerModeBtnStyle}
          title="Компактное окно статуса поверх всех окон"
        >
          {overlayError ? 'Окно недоступно' : 'Окно поверх'}
        </button>
      )}
    </div>
  );
}

const headerModeBtnStyle: CSSProperties = {
  background: 'transparent',
  border: '1px solid var(--on-dark-soft, #ffffff55)',
  color: 'var(--on-dark)',
  fontFamily: 'var(--sans)',
  fontSize: 13,
  padding: '6px 12px',
  cursor: 'pointer',
};

function RecordingDot() {
  return (
    <span
      aria-hidden="true"
      style={{
        display: 'inline-block',
        width: 8,
        height: 8,
        borderRadius: '50%',
        background: 'var(--accent)',
      }}
    />
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
    fontFamily: 'var(--sans)',
    fontSize: 14,
    fontWeight: 500,
    textDecoration: 'none',
    color: isActive ? 'var(--accent-deep)' : 'var(--ink-soft)',
    background: isActive ? 'var(--accent-tint)' : 'transparent',
    borderLeft: isActive ? '3px solid var(--accent)' : '3px solid transparent',
  };
}
