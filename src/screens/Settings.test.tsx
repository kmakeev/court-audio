import { describe, it, expect, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { SettingsScreen } from './Settings';
import { setInvoke } from '../test/tauriMock';
import { settingsFixture } from '../test/fixtures';

function renderSettings() {
  return render(
    <MemoryRouter>
      <SettingsScreen />
    </MemoryRouter>,
  );
}

describe('SettingsScreen', () => {
  it('загружает значения из реестра и сохраняет их', async () => {
    const saved = vi.fn();
    setInvoke('get_settings', () => settingsFixture());
    setInvoke('save_settings', (args) => {
      saved(args);
    });
    renderSettings();

    // Поле частоты заполнено дефолтом из реестра.
    expect(await screen.findByDisplayValue('44100')).toBeInTheDocument();

    fireEvent.click(screen.getByText('Сохранить'));
    await waitFor(() => expect(saved).toHaveBeenCalledTimes(1));
    // Команде передаётся полный объект настроек.
    expect(saved.mock.calls[0][0]).toHaveProperty('settings');
  });

  it('валидирует обязательный URL сервера при авто-выгрузке', async () => {
    setInvoke('get_settings', () => settingsFixture());
    setInvoke('save_settings', () => undefined);
    renderSettings();

    const url = await screen.findByLabelText('URL сервера ex_system');
    fireEvent.change(url, { target: { value: '' } });

    expect(
      await screen.findByText('URL сервера обязателен при авто-выгрузке'),
    ).toBeInTheDocument();
    const save = screen.getByText('Сохранить').closest('button');
    expect(save).toBeDisabled();
  });

  it('многоканал: включение без дорожек — ошибка, добавление дорожки с ролью — валидно', async () => {
    const saved = vi.fn();
    setInvoke('get_settings', () => settingsFixture());
    setInvoke('save_settings', (args) => {
      saved(args);
    });
    setInvoke('list_audio_devices', () => [
      { name: 'Микрофон зала', is_default: true, default_sample_rate_hz: 44100, default_channels: 2, configs: [] },
    ]);
    renderSettings();

    // Включаем многоканальный режим.
    fireEvent.click(await screen.findByText('Включить многоканальный захват'));
    // Пока дорожек нет — валидация запрещает сохранение.
    expect(
      await screen.findByText('Добавьте хотя бы одну дорожку или выключите многоканал'),
    ).toBeInTheDocument();
    expect(screen.getByText('Сохранить').closest('button')).toBeDisabled();

    // Добавляем дорожку — роль по умолчанию из справочника (judge).
    fireEvent.click(screen.getByText('Добавить дорожку'));
    const roleSelect = await screen.findByLabelText('Роль дорожки 1');
    // Кастомный combobox дизайн-системы: текущее значение — в тексте триггера.
    expect(roleSelect).toHaveTextContent('judge');

    // Теперь форма валидна и сохраняется с картой дорожек.
    const save = screen.getByText('Сохранить');
    await waitFor(() => expect(save.closest('button')).not.toBeDisabled());
    fireEvent.click(save);
    await waitFor(() => expect(saved).toHaveBeenCalledTimes(1));
    const payload = saved.mock.calls[0][0].settings;
    expect(payload.audio.multichannel.enabled).toBe(true);
    expect(payload.audio.tracks).toHaveLength(1);
    expect(payload.audio.tracks[0].role).toBe('judge');
  });
});
