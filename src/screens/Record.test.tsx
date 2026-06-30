import { describe, it, expect, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  setInvoke('capture_status', () => ({
    state: 'idle',
    started_at_unix_ms: null,
    output_dir: null,
    segment_count: 0,
  }));
  setInvoke('start_monitor', () => undefined);
  setInvoke('stop_monitor', () => undefined);
  // Этап 05 — пикер дела на экране «Запись».
  setInvoke('get_case_cache_status', () => ({
    synced_at_unix_ms: null,
    is_fresh: false,
    record_count: 0,
    scope: 'court_docket',
  }));
  setInvoke('search_cases', () => []);
  setInvoke('bind_session_case', () => undefined);
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
    // Статус дублируется (шапка + рядом с метром) — поэтому несколько вхождений.
    expect((await screen.findAllByText('Идёт запись')).length).toBeGreaterThan(0);

    await act(async () => {
      emitEvent('capture_state', { state: 'paused' });
    });
    expect((await screen.findAllByText('Пауза')).length).toBeGreaterThan(0);
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
      emitEvent('audio_level', { channels: [{ peak: 1.0, rms: 0.5 }] });
    });

    // Шкала логарифмическая (дБFS): rms 0.5 ≈ -6 дБFS → ~90% (важно, что > 0).
    const meter = screen.getByRole('meter');
    expect(Number(meter.getAttribute('aria-valuenow'))).toBeGreaterThan(0);
    expect(await screen.findByText('Клиппинг')).toBeInTheDocument();
  });

  it('рисует по индикатору на каждый канал (многоканал)', async () => {
    wireDefaults();
    renderRecord();
    await screen.findByText('Готов к записи');

    await act(async () => {
      emitEvent('audio_level', {
        channels: [
          { peak: 0.5, rms: 0.3 },
          { peak: 0.1, rms: 0.05 },
        ],
      });
    });
    expect(screen.getAllByRole('meter')).toHaveLength(2);
    expect(screen.getByLabelText('Уровень канала 1')).toBeInTheDocument();
    expect(screen.getByLabelText('Уровень канала 2')).toBeInTheDocument();
  });

  it('останавливает запись только после подтверждения в модальном окне', async () => {
    const stopped = vi.fn();
    wireDefaults();
    setInvoke('stop_capture', () => {
      stopped();
      return [];
    });
    renderRecord();
    await screen.findByText('Готов к записи');
    await act(async () => {
      emitEvent('capture_state', { state: 'recording' });
    });

    // Клик «Стоп» открывает подтверждение, а не останавливает сразу.
    fireEvent.click(screen.getByText('■ Стоп'));
    expect(await screen.findByText('Остановить запись?')).toBeInTheDocument();
    expect(stopped).not.toHaveBeenCalled();

    fireEvent.click(screen.getByText('Остановить'));
    await waitFor(() => expect(stopped).toHaveBeenCalledTimes(1));
  });

  it('показывает индикатор сохранения сразу после подтверждения стопа', async () => {
    wireDefaults();
    // Стоп «висит» (финализация идёт) — индикатор должен появиться, не
    // дожидаясь backend-события capture_state.
    let resolveStop: ((v: unknown) => void) | null = null;
    setInvoke('stop_capture', () => new Promise((res) => (resolveStop = res)));
    renderRecord();
    await screen.findByText('Готов к записи');
    await act(async () => {
      emitEvent('capture_state', { state: 'recording' });
    });

    // Подтверждаем остановку в модальном окне.
    fireEvent.click(screen.getByText('■ Стоп'));
    fireEvent.click(await screen.findByText('Остановить'));

    expect(await screen.findByText('Сохранение записи…')).toBeInTheDocument();
    expect(screen.getByRole('progressbar')).toBeInTheDocument();
    // Кнопка «Старт» в фазе сохранения не показывается.
    expect(screen.queryByText('● Старт записи')).not.toBeInTheDocument();

    // Завершаем финализацию — индикатор исчезает.
    await act(async () => {
      resolveStop?.([]);
    });
    await waitFor(() =>
      expect(screen.queryByText('Сохранение записи…')).not.toBeInTheDocument(),
    );
  });

  it('не предлагает восстановить текущую идущую сессию', async () => {
    wireDefaults([
      { dir: '/data/recordings/session-current', completed_segments: 1, already_recovered: false },
    ]);
    // Эта же сессия — активная (идёт запись в фоне).
    setInvoke('capture_status', () => ({
      state: 'recording',
      started_at_unix_ms: Date.now(),
      output_dir: '/data/recordings/session-current',
      segment_count: 1,
    }));
    renderRecord();

    expect((await screen.findAllByText('Идёт запись')).length).toBeGreaterThan(0);
    expect(screen.queryByText('Найдена незавершённая сессия')).not.toBeInTheDocument();
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

  it('восстанавливает статус идущей записи при монтировании', async () => {
    wireDefaults();
    // Запись уже идёт в фоне (после перехода между вкладками).
    setInvoke('capture_status', () => ({
      state: 'recording',
      started_at_unix_ms: Date.now() - 5000,
      output_dir: '/data/recordings/session-1',
      segment_count: 2,
    }));
    renderRecord();

    expect((await screen.findAllByText('Идёт запись')).length).toBeGreaterThan(0);
    expect(await screen.findByText(/session-1/)).toBeInTheDocument();
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

  it('привязывает дело к стартовавшей сессии (ручной ввод → bind_session_case)', async () => {
    wireDefaults();
    const bind = vi.fn();
    setInvoke('start_capture', () => ({
      sample_rate_hz: 44100,
      channels: 1,
      output_dir: '/data/recordings/session-new',
    }));
    setInvoke('bind_session_case', (args) => {
      bind(args);
      return undefined;
    });
    renderRecord();
    await screen.findByText('Готов к записи');

    // Кэш пуст (record_count: 0) → пикер уже в режиме ручного ввода.
    const numberField = await screen.findByLabelText('№ дела');
    act(() => {
      fireEvent.change(numberField, { target: { value: '№ 7-7/2026' } });
    });

    // Старт записи привязывает выбранное дело к новой сессии.
    fireEvent.click(screen.getByText('● Старт записи'));
    await waitFor(() => expect(bind).toHaveBeenCalled());
    const arg = bind.mock.calls[0][0] as {
      dir: string;
      binding: { kind: string; raw_number: string };
    };
    expect(arg.dir).toBe('/data/recordings/session-new');
    expect(arg.binding.kind).toBe('manual');
    expect(arg.binding.raw_number).toBe('№ 7-7/2026');
  });
});
