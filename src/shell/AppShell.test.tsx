import { describe, it, expect, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { AppShell } from './AppShell';
import { AuthProvider } from '../lib/auth-context';
import { setInvoke } from '../test/tauriMock';
import { authStatusFixture } from '../test/fixtures';

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
