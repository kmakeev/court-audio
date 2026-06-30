import { describe, it, expect } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { RecordScreen } from './Record';
import { setInvoke, emitEvent } from '../test/tauriMock';
import { settingsFixture } from '../test/fixtures';

function wireDefaults(recoverable: unknown[] = []) {
  setInvoke('list_audio_devices', () => [
    { name: 'Микрофон зала', is_default: true, default_sample_rate_hz: 44100, default_channels: 1, configs: [] },
  ]);
  setInvoke('get_settings', () => settingsFixture());
  setInvoke('scan_recoverable', () => recoverable);
}

function renderRecord() {
  return render(
    <MemoryRouter>
      <RecordScreen />
    </MemoryRouter>,
  );
}

describe('RecordScreen', () => {
  it('переключает статус по событию capture_state', async () => {
    wireDefaults();
    renderRecord();
    await screen.findByText('Готов к записи');

    await act(async () => {
      emitEvent('capture_state', { state: 'recording' });
    });
    expect(await screen.findByText('Идёт запись')).toBeInTheDocument();

    await act(async () => {
      emitEvent('capture_state', { state: 'paused' });
    });
    expect(await screen.findByText('Пауза')).toBeInTheDocument();
  });

  it('индикатор уровня реагирует на audio_level и показывает клиппинг', async () => {
    wireDefaults();
    renderRecord();
    await screen.findByText('Готов к записи');

    // Перейти в запись, затем протолкнуть пиковый уровень.
    await act(async () => {
      emitEvent('capture_state', { state: 'recording' });
    });
    await act(async () => {
      emitEvent('audio_level', { peak: 1.0, rms: 0.5 });
    });

    const meter = screen.getByRole('meter');
    expect(meter).toHaveAttribute('aria-valuenow', '50');
    expect(await screen.findByText('Клиппинг')).toBeInTheDocument();
  });

  it('показывает баннеры по reliability_warning', async () => {
    wireDefaults();
    renderRecord();
    await screen.findByText('Готов к записи');

    await act(async () => {
      emitEvent('reliability_warning', { kind: 'device_lost' });
    });
    // CriticalNotice префиксует заголовок значком — матчим подстрокой.
    expect(await screen.findByText(/Устройство ввода пропало/)).toBeInTheDocument();

    await act(async () => {
      emitEvent('reliability_warning', { kind: 'disk_critical', free_mb: 100 });
    });
    expect(await screen.findByText(/Критически мало места на диске/)).toBeInTheDocument();
  });

  it('показывает баннер восстановления по scan_recoverable', async () => {
    wireDefaults([{ dir: '/data/recordings/session-1', completed_segments: 3, already_recovered: false }]);
    renderRecord();

    await waitFor(() =>
      expect(screen.getByText('Найдена незавершённая сессия')).toBeInTheDocument(),
    );
    expect(screen.getByText('Продолжить')).toBeInTheDocument();
    expect(screen.getByText('Закрыть')).toBeInTheDocument();
  });
});
