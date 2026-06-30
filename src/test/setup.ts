// Глобальная настройка vitest для компонентных тестов (этап 04): jest-dom
// матчеры + мок Tauri-моста (`@tauri-apps/api/core` и `/event`).
import '@testing-library/jest-dom';
import { beforeEach, vi } from 'vitest';
import { resetTauriMock } from './tauriMock';

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

beforeEach(() => {
  resetTauriMock();
});
