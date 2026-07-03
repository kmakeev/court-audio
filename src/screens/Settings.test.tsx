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

describe('SettingsScreen (оператор)', () => {
  it('сохраняет оператор-секцию: устройство выбирается из списка', async () => {
    const saved = vi.fn();
    setInvoke('get_settings', () => settingsFixture());
    setInvoke('list_audio_devices', () => [
      { name: 'Микрофон зала', is_default: true, default_sample_rate_hz: 44100, default_channels: 1, configs: [] },
    ]);
    setInvoke('save_settings', (args) => {
      saved(args);
      return { kind: 'saved' };
    });
    renderSettings();

    // Устройство ввода — выпадающий список, не текстовое поле.
    const device = await screen.findByLabelText('Устройство ввода');
    fireEvent.click(device);
    fireEvent.click(await screen.findByText('Микрофон зала · по умолчанию'));

    fireEvent.click(screen.getByText('Сохранить'));
    await waitFor(() => expect(saved).toHaveBeenCalledTimes(1));
    // Команде передаётся полный объект настроек + флаг подтверждения.
    expect(saved.mock.calls[0][0]).toHaveProperty('settings');
    expect(saved.mock.calls[0][0]).toHaveProperty('confirmDangerous', false);
    expect(saved.mock.calls[0][0].settings.audio.device).toBe('Микрофон зала');
  });

  it('справочники разметки (роли/категории) правятся оператором', async () => {
    const saved = vi.fn();
    setInvoke('get_settings', () => settingsFixture());
    setInvoke('save_settings', (args) => {
      saved(args);
      return { kind: 'saved' };
    });
    renderSettings();

    const roles = await screen.findByLabelText('Роли говорящих (через запятую)');
    const categories = await screen.findByLabelText('Категории закладок (через запятую)');

    fireEvent.change(roles, { target: { value: 'judge, defense, expert' } });
    fireEvent.change(categories, { target: { value: 'Закладка, Реплика' } });

    fireEvent.click(screen.getByText('Сохранить'));
    await waitFor(() => expect(saved).toHaveBeenCalledTimes(1));
    const payload = saved.mock.calls[0][0].settings;
    expect(payload.audio.roles).toEqual(['judge', 'defense', 'expert']);
    expect(payload.markers.categories).toEqual(['Закладка', 'Реплика']);
  });

  it('админ-параметры на экране оператора не показываются', async () => {
    setInvoke('get_settings', () => settingsFixture());
    renderSettings();

    // Дожидаемся загрузки оператор-секции.
    expect(await screen.findByLabelText('Устройство ввода')).toBeInTheDocument();
    // Инфраструктурные/безопасностные поля здесь отсутствуют — только на «Администрировании».
    expect(screen.queryByLabelText('URL сервера ex_system')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Размер чанка, МБ')).not.toBeInTheDocument();
    expect(screen.queryByText('Администратор · инфраструктура и безопасность')).not.toBeInTheDocument();
  });
});
