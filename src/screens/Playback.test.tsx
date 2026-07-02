import { describe, it, expect, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { PlaybackScreen } from './Playback';
import { setInvoke, emitEvent } from '../test/tauriMock';
import { playerSessionInfoFixture, settingsFixture } from '../test/fixtures';

const DIR = '/data/recordings/session-1700000000000';

function wireDefaults(overSession: Parameters<typeof playerSessionInfoFixture>[0] = {}) {
  setInvoke('get_settings', () => settingsFixture());
  setInvoke('player_open_session', () => playerSessionInfoFixture(overSession));
  setInvoke('player_play', () => undefined);
  setInvoke('player_pause', () => undefined);
  setInvoke('player_seek', () => undefined);
  setInvoke('player_set_rate', () => undefined);
  setInvoke('player_set_volume', () => undefined);
  setInvoke('player_select_track', () => undefined);
  setInvoke('player_close', () => undefined);
}

function renderPlayback() {
  return render(
    <MemoryRouter initialEntries={[`/sessions/${encodeURIComponent(DIR)}/listen`]}>
      <Routes>
        <Route path="sessions/:dir/listen" element={<PlaybackScreen />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('PlaybackScreen', () => {
  it('открывает сессию по dir из URL и рендерит оглавление меток/ролей', async () => {
    wireDefaults();
    const open = vi.fn(() => playerSessionInfoFixture());
    setInvoke('player_open_session', open);
    renderPlayback();

    await waitFor(() => expect(open).toHaveBeenCalledWith({ dir: DIR }));
    expect(await screen.findByText('Инцидент')).toBeInTheDocument();
    expect(screen.getByText('шум в зале')).toBeInTheDocument();
    expect(screen.getByText('judge')).toBeInTheDocument();
  });

  it('показывает номер дела и дату/время записи в шапке', async () => {
    wireDefaults();
    renderPlayback();

    expect(await screen.findByText('№ 1-123/2026, Иванов И.И.')).toBeInTheDocument();
    expect(screen.getByText(new Date(1_700_000_000_000).toLocaleString('ru-RU'))).toBeInTheDocument();
  });

  it('клик по метке в оглавлении зовёт player_seek с id метки', async () => {
    wireDefaults();
    const seek = vi.fn(() => undefined);
    setInvoke('player_seek', seek);
    renderPlayback();

    const row = await screen.findByText('Инцидент');
    await act(async () => {
      fireEvent.click(row);
    });
    await waitFor(() =>
      expect(seek).toHaveBeenCalledWith({ to: { kind: 'marker', id: 'm1' } }),
    );
  });

  it('переход по оглавлению до старта воспроизведения сдвигает позицию (эмулируем эмит бэкендом player_position)', async () => {
    wireDefaults();
    // Реальный player_seek эмитит player_position синхронно даже когда
    // плеер не играет (Stopped/Paused) — иначе UI не узнает новую позицию.
    // 5000мс — специально не совпадает ни с одним offset_ms в фикстуре
    // (метка 1000мс, роль 2000–3000мс), чтобы не столкнуться с их же
    // таймкодами в оглавлении.
    setInvoke('player_seek', () => {
      emitEvent('player_position', { position_ms: 5_000, duration_ms: 125_000, state: 'stopped' });
    });
    renderPlayback();

    const row = await screen.findByText('Инцидент');
    await act(async () => {
      fireEvent.click(row);
    });
    expect(await screen.findByText('00:00:05')).toBeInTheDocument();
  });

  it('показывает номер дела из JSON-привязки (не сырой JSON)', async () => {
    wireDefaults({
      adjudication_ref: JSON.stringify({
        kind: 'manual',
        raw_number: '№ 2-456/2026',
        raw_fio: 'Петров П.П.',
      }),
    });
    renderPlayback();

    expect(await screen.findByText('№ 2-456/2026, Петров П.П.')).toBeInTheDocument();
    expect(screen.queryByText(/"kind":"manual"/)).not.toBeInTheDocument();
  });

  it('подсвечивает текущую запись оглавления по позиции воспроизведения — сразу, без лага', async () => {
    wireDefaults();
    renderPlayback();
    const roleRow = (await screen.findByText('judge')).closest('button');
    const markerRow = screen.getByText('Инцидент').closest('button');
    expect(roleRow).not.toBeNull();
    expect(markerRow).not.toBeNull();

    // До первой метки (offset_ms=1000) — ни одна запись не текущая.
    expect(markerRow).not.toHaveAttribute('aria-current');
    expect(roleRow).not.toHaveAttribute('aria-current');

    // Между меткой (1000мс) и интервалом роли (2000мс) — текущая метка.
    // Подсветка — чистая производная от positionMs, обновляется в том же
    // рендере, без задержки.
    await act(async () => {
      emitEvent('player_position', { position_ms: 1_500, duration_ms: 125_000, state: 'playing' });
    });
    expect(markerRow).toHaveAttribute('aria-current', 'true');
    expect(roleRow).not.toHaveAttribute('aria-current');

    // После начала интервала роли (2000мс) — текущий уже интервал роли.
    await act(async () => {
      emitEvent('player_position', { position_ms: 2_500, duration_ms: 125_000, state: 'playing' });
    });
    expect(roleRow).toHaveAttribute('aria-current', 'true');
    expect(markerRow).not.toHaveAttribute('aria-current');
  });

  it('подсветка не «замерзает»: продолжает следовать за позицией после возобновления воспроизведения', async () => {
    // Регрессия конкретно на дебаунс-реализацию: подсветка ставилась через
    // `setTimeout`, чей cleanup срабатывал на каждое изменение `positionMs` —
    // во время непрерывных тиков (событие каждые ~200мс) таймер отменялся
    // раньше, чем успевал сработать, и подсветка навсегда оставалась на
    // значении, выбранном ДО возобновления воспроизведения.
    wireDefaults();
    setInvoke('player_seek', () => {
      emitEvent('player_position', { position_ms: 1_000, duration_ms: 125_000, state: 'stopped' });
    });
    renderPlayback();
    const markerRow = (await screen.findByText('Инцидент')).closest('button');
    const roleRow = screen.getByText('judge').closest('button');

    // Пауза: кликаем по метке (offset 1000мс) — она подсвечивается.
    fireEvent.click(markerRow!);
    expect(markerRow).toHaveAttribute('aria-current', 'true');

    // Возобновили воспроизведение — приходит частая серия тиков позиции,
    // как реальный эмиттер (`player.position_update_hz`), далеко за пределы
    // и метки, и интервала роли.
    for (const positionMs of [1_200, 1_600, 2_200, 2_800, 3_500]) {
      await act(async () => {
        emitEvent('player_position', { position_ms: positionMs, duration_ms: 125_000, state: 'playing' });
      });
    }

    // Подсветка обязана перейти на интервал роли, а не оставаться на метке.
    expect(roleRow).toHaveAttribute('aria-current', 'true');
    expect(markerRow).not.toHaveAttribute('aria-current');
  });

  it('клик по записи оглавления подсвечивает её сразу, не дожидаясь ответа ядра (без «мигания» прежней записи)', async () => {
    wireDefaults();
    // player_seek никогда не резолвится — если бы подсветка зависела от
    // ответа ядра/события player_position, клик не изменил бы её вовсе.
    setInvoke('player_seek', () => new Promise(() => {}));
    renderPlayback();
    const roleRow = (await screen.findByText('judge')).closest('button');
    const markerRow = screen.getByText('Инцидент').closest('button');

    fireEvent.click(roleRow!);
    expect(roleRow).toHaveAttribute('aria-current', 'true');
    expect(markerRow).not.toHaveAttribute('aria-current');
  });

  it('клик по записи не переключается на предыдущую, когда эхо-событие ядра приходит на 1мс раньше её точного смещения', async () => {
    // Регрессия: ядро округляет позицию через два усекающих деления
    // (мс→фрейм→мс), поэтому после сика на паузе/до старта эхо-событие
    // `player_position` может сообщить позицию чуть МЕНЬШЕ точного offset_ms
    // самой метки — без допуска в сравнении подсветка «съезжала» на
    // предыдущую запись оглавления.
    wireDefaults();
    // Метка m1 из фикстуры — offset_ms: 1000. Эмулируем реальное поведение
    // player_seek: ядро отвечает эхо-событием с позицией на 1мс меньше.
    setInvoke('player_seek', () => {
      emitEvent('player_position', { position_ms: 999, duration_ms: 125_000, state: 'stopped' });
    });
    renderPlayback();
    const markerRow = (await screen.findByText('Инцидент')).closest('button');
    const roleRow = screen.getByText('judge').closest('button');

    fireEvent.click(markerRow!);
    expect(markerRow).toHaveAttribute('aria-current', 'true');
    expect(roleRow).not.toHaveAttribute('aria-current');
  });

  it('кнопка play/pause вызывает соответствующие команды', async () => {
    wireDefaults();
    const play = vi.fn(() => undefined);
    const pause = vi.fn(() => undefined);
    setInvoke('player_play', play);
    setInvoke('player_pause', pause);
    renderPlayback();

    const playBtn = await screen.findByText('▶ Играть');
    await act(async () => {
      fireEvent.click(playBtn);
    });
    await waitFor(() => expect(play).toHaveBeenCalled());

    await act(async () => {
      emitEvent('player_position', { position_ms: 1000, duration_ms: 125000, state: 'playing' });
    });
    const pauseBtn = await screen.findByText('❚❚ Пауза');
    await act(async () => {
      fireEvent.click(pauseBtn);
    });
    await waitFor(() => expect(pause).toHaveBeenCalled());
  });

  it('событие player_position обновляет отображаемый таймкод', async () => {
    wireDefaults();
    renderPlayback();
    await screen.findByText('Инцидент');

    await act(async () => {
      emitEvent('player_position', { position_ms: 65_000, duration_ms: 125_000, state: 'playing' });
    });
    expect(await screen.findByText('00:01:05')).toBeInTheDocument();
  });

  it('хоткей стрелки вправо перематывает на seek_step_seconds из настроек', async () => {
    wireDefaults();
    const seek = vi.fn(() => undefined);
    setInvoke('player_seek', seek);
    renderPlayback();
    await screen.findByText('Инцидент');

    await act(async () => {
      emitEvent('player_position', { position_ms: 10_000, duration_ms: 125_000, state: 'playing' });
    });
    await screen.findByText('00:00:10');

    await act(async () => {
      fireEvent.keyDown(window, { key: 'ArrowRight' });
    });
    // seek_step_seconds фикстуры — 15 → 10с + 15с = 25с = 25000мс.
    await waitFor(() => expect(seek).toHaveBeenCalledWith({ to: { kind: 'ms', ms: 25_000 } }));
  });

  it('хоткей «пробел» переключает play/pause', async () => {
    wireDefaults();
    const play = vi.fn(() => undefined);
    setInvoke('player_play', play);
    renderPlayback();
    await screen.findByText('Инцидент');

    await act(async () => {
      fireEvent.keyDown(window, { key: ' ' });
    });
    await waitFor(() => expect(play).toHaveBeenCalled());
  });

  it('выбор дорожки/микса вызывает player_select_track', async () => {
    wireDefaults({
      tracks: [
        { track_id: 0, role: 'judge', label: 'Судья' },
        { track_id: 1, role: 'defense', label: 'Защита' },
      ],
    });
    const select = vi.fn(() => undefined);
    setInvoke('player_select_track', select);
    renderPlayback();

    const trigger = await screen.findByLabelText('Дорожка');
    await act(async () => {
      fireEvent.click(trigger);
    });
    const mixOption = await screen.findByText('Микс всех дорожек');
    await act(async () => {
      fireEvent.click(mixOption);
    });
    await waitFor(() => expect(select).toHaveBeenCalledWith({ selector: { kind: 'mix' } }));
  });

  it('показывает ошибку, если сессию не удалось открыть', async () => {
    wireDefaults();
    setInvoke('player_open_session', () => {
      throw new Error('нет сегментов');
    });
    renderPlayback();

    expect(await screen.findByText(/Ошибка: нет сегментов/)).toBeInTheDocument();
  });
});
