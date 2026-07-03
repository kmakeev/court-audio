import { describe, it, expect, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { CompactOverlayRoot } from './CompactOverlay';
import { emitEvent, setInvoke } from '../test/tauriMock';
import { settingsFixture } from '../test/fixtures';

// Идёт запись, проигрыватель не открыт.
function mockRecordingOnly() {
  setInvoke('capture_status', () => ({
    state: 'recording',
    started_at_unix_ms: Date.now() - 4000,
    output_dir: '/data/rec',
    segment_count: 2,
  }));
  setInvoke('player_status', () => ({
    active: false,
    position_ms: 0,
    duration_ms: 0,
    state: 'stopped',
  }));
}

describe('CompactOverlay — режим записи', () => {
  it('показывает состояние записи и хронометр', async () => {
    mockRecordingOnly();
    render(<CompactOverlayRoot />);
    expect(await screen.findByText('Идёт запись')).toBeInTheDocument();
    expect(screen.getByLabelText('Хронометраж записи')).toBeInTheDocument();
  });

  it('кнопка закрытия вызывает команду ядра close_compact_overlay', async () => {
    const close = vi.fn();
    setInvoke('capture_status', () => ({
      state: 'idle',
      started_at_unix_ms: null,
      output_dir: null,
      segment_count: 0,
    }));
    setInvoke('player_status', () => ({ active: false, position_ms: 0, duration_ms: 0, state: 'stopped' }));
    setInvoke('close_compact_overlay', () => close());
    render(<CompactOverlayRoot />);
    fireEvent.click(await screen.findByLabelText('Закрыть окно статуса'));
    await waitFor(() => expect(close).toHaveBeenCalled());
  });
});

describe('CompactOverlay — режим воспроизведения', () => {
  function mockPlaybackActive() {
    setInvoke('capture_status', () => ({
      state: 'idle',
      started_at_unix_ms: null,
      output_dir: null,
      segment_count: 0,
    }));
    setInvoke('player_status', () => ({
      active: true,
      position_ms: 5000,
      duration_ms: 125000,
      state: 'playing',
    }));
    setInvoke('get_settings', () => settingsFixture());
  }

  it('при активном плейбеке показывает позицию и транспорт', async () => {
    mockPlaybackActive();
    render(<CompactOverlayRoot />);
    expect(await screen.findByText('▶ Воспроизведение')).toBeInTheDocument();
    // Позиция/длительность.
    expect(screen.getByLabelText('Позиция воспроизведения')).toHaveTextContent('00:00:05');
    expect(screen.getByLabelText('Позиция воспроизведения')).toHaveTextContent('00:02:05');
    // Транспорт: пауза (играет) + перемотка.
    expect(screen.getByLabelText('Пауза')).toBeInTheDocument();
    expect(screen.getByLabelText('Назад')).toBeInTheDocument();
    expect(screen.getByLabelText('Вперёд')).toBeInTheDocument();
  });

  it('кнопка play/pause и клавиша Пробел управляют плеером', async () => {
    const pause = vi.fn();
    mockPlaybackActive();
    setInvoke('player_pause', () => pause());
    render(<CompactOverlayRoot />);
    // Играет → кнопка ставит на паузу.
    fireEvent.click(await screen.findByLabelText('Пауза'));
    await waitFor(() => expect(pause).toHaveBeenCalledTimes(1));
    // Пробел — тоже пауза (окно в фокусе).
    fireEvent.keyDown(window, { key: ' ' });
    await waitFor(() => expect(pause).toHaveBeenCalledTimes(2));
  });

  it('перемотка вперёд вызывает player_seek со сдвигом на шаг', async () => {
    const seek = vi.fn();
    mockPlaybackActive();
    setInvoke('player_seek', (args) => seek(args));
    render(<CompactOverlayRoot />);
    fireEvent.click(await screen.findByLabelText('Вперёд'));
    // seek_step по умолчанию 15 c → 5000 + 15000 = 20000 мс.
    await waitFor(() =>
      expect(seek).toHaveBeenCalledWith({ to: { kind: 'ms', ms: 20000 } }),
    );
  });

  it('после player_closed возвращается к статусу записи', async () => {
    mockPlaybackActive();
    // Параллельно идёт запись — после закрытия плеера окно покажет её.
    setInvoke('capture_status', () => ({
      state: 'recording',
      started_at_unix_ms: Date.now() - 1000,
      output_dir: '/data/rec',
      segment_count: 1,
    }));
    render(<CompactOverlayRoot />);
    await screen.findByText('▶ Воспроизведение');
    emitEvent('player_closed', null);
    expect(await screen.findByText('Идёт запись')).toBeInTheDocument();
  });
});
