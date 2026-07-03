// Глобальная настройка vitest для компонентных тестов (этап 04): jest-dom
// матчеры + мок Tauri-моста (`@tauri-apps/api/core` и `/event`).
import '@testing-library/jest-dom';
import { beforeEach, vi } from 'vitest';
import { resetTauriMock } from './tauriMock';

// jsdom не реализует `window.matchMedia`; минимальный стаб (этап 10.5) — на
// случай, если компонент/библиотека его вызовет. Адаптивная сетка построена на
// CSS-медиазапросах (не на JS matchMedia), поэтому смоук-тесты его не читают.
if (typeof window !== 'undefined' && !window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}

vi.mock('@tauri-apps/api/core', async () => {
  const mod = await import('./tauriMock');
  return {
    invoke: (cmd: string, args?: unknown) => mod.mockInvoke(cmd, args),
  };
});

vi.mock('@tauri-apps/api/event', async () => {
  const mod = await import('./tauriMock');
  return {
    listen: (name: string, cb: (e: { payload: unknown }) => void) =>
      mod.mockListen(name, cb),
  };
});

// Мок оконного API Tauri (этап 10.6): защита закрытия окна с идущей записью
// использует `getCurrentWindow().onCloseRequested`. В jsdom нативного окна нет —
// подставляем no-op, чтобы компонентные тесты не падали на импорте.
vi.mock('@tauri-apps/api/window', async () => {
  return {
    getCurrentWindow: () => ({
      onCloseRequested: (_cb: unknown) => Promise.resolve(() => {}),
      destroy: () => Promise.resolve(),
    }),
  };
});

beforeEach(() => {
  resetTauriMock();
});
