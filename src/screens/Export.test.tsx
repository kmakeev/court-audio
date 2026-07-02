import { describe, it, expect, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { ExportScreen } from './Export';
import { setInvoke, emitEvent } from '../test/tauriMock';
import { exportSessionInfoFixture, exportResultFixture, settingsFixture } from '../test/fixtures';
import type { Settings } from '../lib/settings';

const DIR = '/data/recordings/session-1700000000000';

function wireDefaults(overSettings: Partial<Settings> = {}) {
  setInvoke('get_settings', () => ({ ...settingsFixture(), ...overSettings }));
  setInvoke('export_session_info', () => exportSessionInfoFixture());
  setInvoke('export_build_package', () => exportResultFixture());
  setInvoke('export_dvd_drive_status', () => null);
}

function renderExport() {
  return render(
    <MemoryRouter initialEntries={[`/sessions/${encodeURIComponent(DIR)}/export`]}>
      <Routes>
        <Route path="sessions/:dir/export" element={<ExportScreen />} />
      </Routes>
    </MemoryRouter>,
  );
}

async function openSelect(label: string) {
  const trigger = await screen.findByLabelText(label);
  await act(async () => {
    fireEvent.click(trigger);
  });
}

describe('ExportScreen', () => {
  it('загружает сессию и показывает дорожки в выборе состава', async () => {
    wireDefaults();
    renderExport();

    await openSelect('Состав пакета');
    const listbox = await screen.findByRole('listbox');
    expect(within(listbox).getByText('Все дорожки')).toBeInTheDocument();
    expect(within(listbox).getByText('Сведённый микс')).toBeInTheDocument();
    expect(within(listbox).getByText('Судья')).toBeInTheDocument();
    expect(within(listbox).getByText('Защита')).toBeInTheDocument();
  });

  it('одноканальная v1-сессия (легаси-роль "single") показывает в составе «Запись», а не внутренний код', async () => {
    // `store::manifest::ManifestStore::resolve_tracks` синтезирует для
    // одноканальных сессий одну дорожку с `role: "single"` и пустой `label`
    // (`crate::audio::tracks::SINGLE_TRACK_ROLE` — внутренний Rust-код, не
    // часть настраиваемого справочника `audio.roles`, никогда не должен
    // просачиваться в UI).
    setInvoke('get_settings', () => settingsFixture());
    setInvoke(
      'export_session_info',
      () =>
        exportSessionInfoFixture({
          tracks: [{ track_id: 0, role: 'single', label: '' }],
        }),
    );
    setInvoke('export_build_package', () => exportResultFixture());
    setInvoke('export_dvd_drive_status', () => null);
    renderExport();

    await openSelect('Состав пакета');
    const listbox = await screen.findByRole('listbox');
    expect(within(listbox).getByText('Запись')).toBeInTheDocument();
    expect(within(listbox).queryByText('single')).not.toBeInTheDocument();
  });

  it('счастливый путь: собирает пакет с выбранным форматом и показывает сводку', async () => {
    wireDefaults();
    const build = vi.fn(() => exportResultFixture());
    setInvoke('export_build_package', build);
    renderExport();

    await screen.findByText('Целостность подтверждена');

    await openSelect('Формат аудио');
    await act(async () => {
      fireEvent.click(await screen.findByText('FLAC (компактнее, без потерь)'));
    });

    await act(async () => {
      fireEvent.click(screen.getByText('Начать экспорт'));
    });

    await waitFor(() =>
      expect(build).toHaveBeenCalledWith({
        dir: DIR,
        composition: { kind: 'all_tracks' },
        format: 'flac',
        destinationDir: null,
        confirmed: false,
      }),
    );

    expect(await screen.findByText('audio/sudya.wav', { exact: false })).toBeInTheDocument();
    expect(screen.getByText('audio/zaschita.wav', { exact: false })).toBeInTheDocument();
  });

  it('политика requires_confirmation: открывает подтверждение и вызывает сборку только после него', async () => {
    wireDefaults({ export: { policy: 'requires_confirmation', default_codec: 'wav_pcm' } });
    const build = vi.fn(() => exportResultFixture());
    setInvoke('export_build_package', build);
    renderExport();

    await screen.findByText('Целостность подтверждена');
    await act(async () => {
      fireEvent.click(screen.getByText('Начать экспорт'));
    });

    expect(await screen.findByText('Подтвердите экспорт')).toBeInTheDocument();
    expect(build).not.toHaveBeenCalled();

    await act(async () => {
      fireEvent.click(screen.getByText('Экспортировать'));
    });

    await waitFor(() =>
      expect(build).toHaveBeenCalledWith(
        expect.objectContaining({ dir: DIR, confirmed: true }),
      ),
    );
  });

  it('политика forbidden: показывает блокирующее уведомление и не даёт запустить экспорт', async () => {
    wireDefaults({ export: { policy: 'forbidden', default_codec: 'wav_pcm' } });
    const build = vi.fn(() => exportResultFixture());
    setInvoke('export_build_package', build);
    renderExport();

    expect(await screen.findByText('Экспорт запрещён администратором станции')).toBeInTheDocument();
    expect(screen.queryByText('Начать экспорт')).not.toBeInTheDocument();
    expect(build).not.toHaveBeenCalled();
  });

  it('показывает прогресс сборки по событию export_progress до появления сводки', async () => {
    wireDefaults();
    setInvoke(
      'export_build_package',
      () =>
        new Promise((resolve) => {
          // Эмит откладываем на макротаск: иначе он происходит синхронно
          // внутри того же вызова, что и `setStep('progress')`, до того как
          // React закоммитит рендер и эффект подписки на `export_progress`
          // успеет сработать — событие было бы потеряно (как реальный
          // Tauri-эмит, который тоже асинхронен относительно вызова команды).
          setTimeout(() => {
            emitEvent('export_progress', { stage: 'joining', percent: 40 });
            setTimeout(() => resolve(exportResultFixture()), 0);
          }, 0);
        }),
    );
    renderExport();
    await screen.findByText('Целостность подтверждена');

    await act(async () => {
      fireEvent.click(screen.getByText('Начать экспорт'));
    });

    await waitFor(() => expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '40'));
    expect(await screen.findByText('Готово')).toBeInTheDocument();
  });

  it('назначение DVD без привода показывает понятную диагностику, папка остаётся доступна', async () => {
    wireDefaults();
    setInvoke('export_dvd_drive_status', () => null);
    renderExport();
    await screen.findByText('Целостность подтверждена');

    await openSelect('Назначение');
    await act(async () => {
      fireEvent.click(await screen.findByText('DVD'));
    });

    expect(await screen.findByText('Привод/утилита прожига не найдены')).toBeInTheDocument();
  });

  it('назначение DVD с найденным приводом: прожигает пакет после сборки и показывает верификацию', async () => {
    wireDefaults();
    setInvoke('export_dvd_drive_status', () => ({ id: '/dev/sr0', label: 'DVD-RW' }));
    const burn = vi.fn(() => ({ verified: true, drive: '/dev/sr0' }));
    setInvoke('export_burn_dvd', burn);
    renderExport();
    await screen.findByText('Целостность подтверждена');

    await openSelect('Назначение');
    await act(async () => {
      fireEvent.click(await screen.findByText('DVD'));
    });
    expect(await screen.findByText('Найден привод: DVD-RW')).toBeInTheDocument();

    await act(async () => {
      fireEvent.click(screen.getByText('Начать экспорт'));
    });
    await screen.findByText('Готово');

    await act(async () => {
      fireEvent.click(screen.getByText('Записать на DVD (DVD-RW)'));
    });

    await waitFor(() =>
      expect(burn).toHaveBeenCalledWith({
        dir: DIR,
        packageDir: exportResultFixture().package_dir,
        driveId: '/dev/sr0',
      }),
    );
    expect(await screen.findByText('DVD прожжён и верифицирован')).toBeInTheDocument();
  });

  it('показывает ошибку сборки и возвращает к форме', async () => {
    wireDefaults();
    setInvoke('export_build_package', () => {
      throw new Error('нет места на диске');
    });
    renderExport();
    await screen.findByText('Целостность подтверждена');

    await act(async () => {
      fireEvent.click(screen.getByText('Начать экспорт'));
    });

    expect(await screen.findByText(/Ошибка: нет места на диске/)).toBeInTheDocument();
    expect(await screen.findByText('Начать экспорт')).toBeInTheDocument();
  });
});
