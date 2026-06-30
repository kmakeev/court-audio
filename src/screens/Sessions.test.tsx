import { describe, it, expect, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { SessionsScreen } from './Sessions';
import { setInvoke } from '../test/tauriMock';
import { sessionViewFixture } from '../test/fixtures';

function renderSessions() {
  return render(
    <MemoryRouter>
      <SessionsScreen />
    </MemoryRouter>,
  );
}

describe('SessionsScreen', () => {
  it('рендерит список сессий из манифеста', async () => {
    setInvoke('list_sessions', () => [
      sessionViewFixture(),
      sessionViewFixture({ id: 'session-2', adjudication_ref: null, upload_status: 'confirmed' }),
    ]);
    renderSessions();

    expect(await screen.findByText('№ 1-123/2026, Иванов И.И.')).toBeInTheDocument();
    expect(screen.getByText('Без привязки к делу')).toBeInTheDocument();
    expect(screen.getByText('Подтверждена')).toBeInTheDocument();
    // Длительность 125 с → 00:02:05 (обе фикстуры — отсюда несколько вхождений).
    expect(screen.getAllByText(/00:02:05/).length).toBeGreaterThan(0);
  });

  it('показывает пустое состояние при отсутствии сессий', async () => {
    setInvoke('list_sessions', () => []);
    renderSessions();

    expect(await screen.findByText('Записанных сессий пока нет')).toBeInTheDocument();
  });

  it('показывает прогресс выгрузки в процентах', async () => {
    setInvoke('list_sessions', () => [
      sessionViewFixture({
        upload_status: 'uploading',
        upload_total_parts: 4,
        upload_sent_parts: 1,
      }),
    ]);
    renderSessions();

    expect(await screen.findByText('Выгружается 25%')).toBeInTheDocument();
  });

  it('показывает ошибку целостности и позволяет повторить выгрузку', async () => {
    const retry = vi.fn(() => undefined);
    setInvoke('list_sessions', () => [
      sessionViewFixture({ upload_status: 'integrity_failed' }),
    ]);
    setInvoke('retry_upload', retry);
    renderSessions();

    expect(await screen.findByText('Ошибка целостности')).toBeInTheDocument();
    await userEvent.click(screen.getByText('Повторить'));
    await waitFor(() =>
      expect(retry).toHaveBeenCalledWith({
        dir: '/data/recordings/session-1700000000000',
      }),
    );
  });

  it('позволяет поставить выгрузку на паузу', async () => {
    const pause = vi.fn(() => undefined);
    setInvoke('list_sessions', () => [sessionViewFixture({ upload_status: 'pending' })]);
    setInvoke('pause_upload', pause);
    renderSessions();

    await userEvent.click(await screen.findByText('Пауза'));
    await waitFor(() =>
      expect(pause).toHaveBeenCalledWith({
        dir: '/data/recordings/session-1700000000000',
      }),
    );
  });
});
