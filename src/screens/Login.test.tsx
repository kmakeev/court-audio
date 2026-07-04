import { describe, it, expect, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { LoginScreen } from './Login';
import { AuthProvider } from '../lib/auth-context';
import { setInvoke } from '../test/tauriMock';
import { authStatusFixture } from '../test/fixtures';
import type { AuthStatus } from '../lib/core';

// Экран входа оператора (этап 10.3): онлайн-вход, ошибки, оффлайн-разблокировка
// по PIN. Сеть/ядро замоканы через tauriMock.

function loggedOut(over: Partial<AuthStatus> = {}): AuthStatus {
  return authStatusFixture({
    operator: null,
    online: false,
    offline_cached: false,
    cache_expires_at_unix_ms: null,
    ...over,
  });
}

function renderLogin() {
  return render(
    <MemoryRouter initialEntries={['/login']}>
      <AuthProvider>
        <Routes>
          <Route path="/login" element={<LoginScreen />} />
          <Route path="/" element={<div>ЭКРАН ЗАПИСИ</div>} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>,
  );
}

describe('LoginScreen', () => {
  it('входит по логину/паролю/PIN и уходит на экран записи', async () => {
    setInvoke('auth_status', () => loggedOut());
    const login = vi.fn(() => authStatusFixture());
    setInvoke('auth_login', login);

    renderLogin();

    fireEvent.change(await screen.findByLabelText('Логин (email)'), {
      target: { value: 'op@court' },
    });
    fireEvent.change(screen.getByLabelText('Пароль'), {
      target: { value: 'secret' },
    });
    fireEvent.change(screen.getByLabelText('PIN для оффлайн-старта'), {
      target: { value: '2468' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Войти' }));

    await waitFor(() => expect(screen.getByText('ЭКРАН ЗАПИСИ')).toBeInTheDocument());
    expect(login).toHaveBeenCalledWith({
      email: 'op@court',
      password: 'secret',
      pin: '2468',
    });
  });

  it('показывает понятную ошибку при неверных данных', async () => {
    setInvoke('auth_status', () => loggedOut());
    setInvoke('auth_login', () => {
      throw new Error('неверный логин или пароль');
    });

    renderLogin();

    fireEvent.change(await screen.findByLabelText('Логин (email)'), {
      target: { value: 'op@court' },
    });
    fireEvent.change(screen.getByLabelText('Пароль'), {
      target: { value: 'bad' },
    });
    fireEvent.change(screen.getByLabelText('PIN для оффлайн-старта'), {
      target: { value: '2468' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Войти' }));

    expect(await screen.findByText(/неверный логин или пароль/)).toBeInTheDocument();
    expect(screen.queryByText('ЭКРАН ЗАПИСИ')).not.toBeInTheDocument();
  });

  it('предлагает оффлайн-разблокировку по PIN при валидном кэше', async () => {
    setInvoke('auth_status', () => loggedOut({ offline_cached: true }));
    const unlock = vi.fn(() => authStatusFixture({ online: false }));
    setInvoke('auth_unlock_offline', unlock);

    renderLogin();

    const pinField = await screen.findByLabelText('PIN');
    fireEvent.change(pinField, { target: { value: '2468' } });
    fireEvent.click(screen.getByRole('button', { name: 'Войти' }));

    await waitFor(() => expect(screen.getByText('ЭКРАН ЗАПИСИ')).toBeInTheDocument());
    expect(unlock).toHaveBeenCalledWith({ pin: '2468' });
  });

  it('сообщает о неверном PIN при оффлайн-разблокировке', async () => {
    setInvoke('auth_status', () => loggedOut({ offline_cached: true }));
    setInvoke('auth_unlock_offline', () => {
      throw new Error('неверный PIN');
    });

    renderLogin();

    const pinField = await screen.findByLabelText('PIN');
    fireEvent.change(pinField, { target: { value: '0000' } });
    fireEvent.click(screen.getByRole('button', { name: 'Войти' }));

    expect(await screen.findByText(/неверный PIN/)).toBeInTheDocument();
    expect(screen.queryByText('ЭКРАН ЗАПИСИ')).not.toBeInTheDocument();
  });

  it('автономный старт (B-001): вход по провижиненному PIN без онлайн-входа', async () => {
    // Изолированный зал: кэша онлайн-сессии нет, но профиль зала провижинен.
    setInvoke('auth_status', () => loggedOut({ autonomous_available: true }));
    const unlock = vi.fn(() => authStatusFixture({ online: false }));
    setInvoke('auth_unlock_autonomous', unlock);

    renderLogin();

    expect(await screen.findByText('Автономный старт')).toBeInTheDocument();
    const pinField = await screen.findByLabelText('PIN');
    fireEvent.change(pinField, { target: { value: '2468' } });
    fireEvent.click(screen.getByRole('button', { name: 'Войти' }));

    await waitFor(() => expect(screen.getByText('ЭКРАН ЗАПИСИ')).toBeInTheDocument());
    expect(unlock).toHaveBeenCalledWith({ pin: '2468' });
  });

  it('показывает только одну форму и переключается по ссылке в обе стороны', async () => {
    setInvoke('auth_status', () => loggedOut({ offline_cached: true }));

    renderLogin();

    // По умолчанию — только оффлайн-карточка (PIN); онлайн-формы нет.
    await screen.findByLabelText('PIN');
    expect(screen.queryByLabelText('Логин (email)')).not.toBeInTheDocument();

    // Ссылка раскрывает онлайн-форму — при этом PIN-форма скрывается.
    fireEvent.click(screen.getByRole('button', { name: 'Войти по учётной записи' }));
    expect(await screen.findByLabelText('Логин (email)')).toBeInTheDocument();
    expect(screen.queryByLabelText('PIN')).not.toBeInTheDocument();

    // Обратная ссылка возвращает к оффлайн-входу по PIN.
    fireEvent.click(screen.getByRole('button', { name: 'Оффлайн-вход по PIN' }));
    expect(await screen.findByLabelText('PIN')).toBeInTheDocument();
    expect(screen.queryByLabelText('Логин (email)')).not.toBeInTheDocument();
  });
});
