import { describe, it, expect, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { HallMode } from './HallMode';
import { RecordingStatusProvider } from '../lib/recording-status';
import { setInvoke } from '../test/tauriMock';

function renderHall(onClose = vi.fn()) {
  setInvoke('capture_status', () => ({
    state: 'recording',
    started_at_unix_ms: Date.now() - 7000,
    output_dir: '/data/rec',
    segment_count: 5,
  }));
  render(
    <RecordingStatusProvider>
      <HallMode onClose={onClose} />
    </RecordingStatusProvider>,
  );
  return onClose;
}

describe('HallMode — режим зала', () => {
  it('показывает крупный статус и хронометр записи', async () => {
    renderHall();
    expect(await screen.findByText('Идёт запись')).toBeInTheDocument();
    // Модальная панель.
    expect(screen.getByRole('dialog', { name: 'Режим зала — статус записи' })).toBeInTheDocument();
    expect(screen.getByLabelText('Хронометраж записи')).toBeInTheDocument();
  });

  it('Esc закрывает панель', async () => {
    const onClose = renderHall();
    await screen.findByText('Идёт запись');
    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it('кнопка «Свернуть» закрывает панель', async () => {
    const onClose = renderHall();
    fireEvent.click(await screen.findByText('Свернуть · Esc'));
    expect(onClose).toHaveBeenCalled();
  });
});
