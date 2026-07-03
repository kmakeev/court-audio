import { describe, it, expect, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { AppShell } from './AppShell';
import { AuthProvider } from '../lib/auth-context';
import { emitEvent, setInvoke } from '../test/tauriMock';
import { authStatusFixture, settingsFixture } from '../test/fixtures';

// Шапка приложения (этап 10.3): ФИО+роль оператора, индикатор связи, смена
// оператора (выход) → экран входа. Идущую запись выход не касается (проверяется
// в Rust-тестах ядра).

function renderShell() {
  return render(
    <MemoryRouter initialEntries={['/']}>
      <AuthProvider>
        <Routes>
          <Route element={<AppShell />}>
            <Route index element={<div>СОДЕРЖИМОЕ</div>} />
          </Route>
          <Route path="/login" element={<div>ЭКРАН ВХОДА</div>} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>,
  );
}

describe('AppShell', () => {
  it('показывает ФИО оператора и статус связи «Онлайн»', async () => {
    setInvoke('auth_status', () => authStatusFixture());
    renderShell();

    expect(await screen.findByText('Иванов И. И.')).toBeInTheDocument();
    expect(screen.getByText('Помощник судьи')).toBeInTheDocument();
    expect(screen.getByText('Онлайн')).toBeInTheDocument();
  });

  it('в оффлайн-режиме показывает статус «Оффлайн»', async () => {
    setInvoke('auth_status', () => authStatusFixture({ online: false }));
    renderShell();

    expect(await screen.findByText('Оффлайн')).toBeInTheDocument();
  });

  it('смена оператора выходит и ведёт на экран входа', async () => {
    setInvoke('auth_status', () => authStatusFixture());
    const logout = vi.fn(() => authStatusFixture({ operator: null }));
    setInvoke('auth_logout', logout);

    renderShell();
    fireEvent.click(await screen.findByRole('button', { name: 'Сменить оператора' }));

    await waitFor(() => expect(screen.getByText('ЭКРАН ВХОДА')).toBeInTheDocument());
    expect(logout).toHaveBeenCalled();
  });
});

// ── Адаптив и глобальный индикатор записи (этап 10.5) ────────────────────────
function renderShellUi(captureState: 'idle' | 'recording') {
  setInvoke('auth_status', () => authStatusFixture());
  setInvoke('get_settings', () => settingsFixture());
  setInvoke('capture_status', () => ({
    state: captureState,
    started_at_unix_ms: captureState === 'idle' ? null : Date.now() - 3000,
    output_dir: captureState === 'idle' ? null : '/data/rec',
    segment_count: 2,
  }));
  return render(
    <MemoryRouter initialEntries={['/']}>
      <AuthProvider>
        <Routes>
          <Route element={<AppShell />}>
            <Route index element={<div>СОДЕРЖИМОЕ</div>} />
          </Route>
        </Routes>
      </AuthProvider>
    </MemoryRouter>,
  );
}

describe('AppShell — адаптив и глобальный индикатор (10.5)', () => {
  it('бургер-кнопка навигации присутствует (видимость — по CSS-медиазапросу)', async () => {
    renderShellUi('idle');
    expect(await screen.findByLabelText('Показать навигацию')).toBeInTheDocument();
  });

  it('индикатор записи в шапке виден при активной сессии', async () => {
    renderShellUi('recording');
    expect(await screen.findByText('Идёт запись')).toBeInTheDocument();
    expect(screen.getByLabelText('Хронометраж записи')).toBeInTheDocument();
  });

  it('в простое хронометр в шапке скрыт', async () => {
    renderShellUi('idle');
    await screen.findByLabelText('Показать навигацию');
    expect(screen.queryByLabelText('Хронометраж записи')).not.toBeInTheDocument();
  });

  it('кнопка «Режим зала» показана (ui.hall_mode.enabled по умолчанию)', async () => {
    renderShellUi('idle');
    expect(await screen.findByText('Режим зала')).toBeInTheDocument();
  });

  it('«Окно поверх» появляется после включения в настройках без перезапуска', async () => {
    // Оверлей выключен → кнопки нет; после сохранения (событие settings_saved)
    // оболочка перечитывает реестр и показывает кнопку.
    let overlayEnabled = false;
    setInvoke('auth_status', () => authStatusFixture());
    setInvoke('capture_status', () => ({
      state: 'idle',
      started_at_unix_ms: null,
      output_dir: null,
      segment_count: 0,
    }));
    setInvoke('get_settings', () => {
      const s = settingsFixture();
      s.ui.compact_overlay.enabled = overlayEnabled;
      return s;
    });
    render(
      <MemoryRouter initialEntries={['/']}>
        <AuthProvider>
          <Routes>
            <Route element={<AppShell />}>
              <Route index element={<div>СОДЕРЖИМОЕ</div>} />
            </Route>
          </Routes>
        </AuthProvider>
      </MemoryRouter>,
    );

    await screen.findByText('Режим зала');
    expect(screen.queryByText('Окно поверх')).not.toBeInTheDocument();

    // Администратор включил оверлей и сохранил — ядро эмитит `settings_saved`.
    overlayEnabled = true;
    emitEvent('settings_saved', null);

    expect(await screen.findByText('Окно поверх')).toBeInTheDocument();
  });
});
