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
});
