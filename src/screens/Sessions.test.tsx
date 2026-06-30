import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
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
});
