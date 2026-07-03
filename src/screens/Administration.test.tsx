import { describe, it, expect, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { AdministrationScreen } from './Administration';
import { resetTauriMock, setInvoke } from '../test/tauriMock';
import { settingsFixture } from '../test/fixtures';

function renderAdmin() {
  return render(
    <MemoryRouter>
      <AdministrationScreen />
    </MemoryRouter>,
  );
}

function baseMocks(over: { unlocked?: boolean; provisioned?: boolean } = {}) {
  setInvoke('get_settings', () => settingsFixture());
  setInvoke('list_audio_devices', () => []);
  setInvoke('get_settings_audit', () => []);
  setInvoke('admin_status', () => ({
    provisioned: over.provisioned ?? true,
    unlocked: over.unlocked ?? false,
    required: true,
  }));
}

describe('AdministrationScreen', () => {
  beforeEach(() => resetTauriMock());

  it('пока не разблокировано — параметры/профиль/журнал скрыты, есть «Разблокировать»', async () => {
    baseMocks({ unlocked: false });
    renderAdmin();

    expect(await screen.findByText('Разблокировать')).toBeInTheDocument();
    // Никаких операций без PIN: ни полей, ни экспорта, ни кнопки сохранения.
    expect(screen.queryByLabelText('URL сервера ex_system')).not.toBeInTheDocument();
    expect(screen.queryByText('Сохранить')).not.toBeInTheDocument();
    expect(screen.queryByText('Экспортировать профиль')).not.toBeInTheDocument();
  });

  it('разблокировка по PIN делает секцию редактируемой', async () => {
    baseMocks({ unlocked: false });
    setInvoke('admin_unlock', () => ({ provisioned: true, unlocked: true, required: true }));
    renderAdmin();

    const pin = await screen.findByLabelText('Админ-PIN');
    fireEvent.change(pin, { target: { value: '2468' } });
    fireEvent.click(screen.getByText('Разблокировать'));

    // После разблокировки URL редактируем и появляется «Сохранить».
    await waitFor(() => expect(screen.getByLabelText('URL сервера ex_system')).not.toBeDisabled());
    expect(screen.getByText('Сохранить')).toBeInTheDocument();
    expect(screen.getByText('Админ-доступ разблокирован')).toBeInTheDocument();
    // У сложных параметров есть подсказки «ⓘ».
    expect(screen.getAllByLabelText('Что это?').length).toBeGreaterThan(0);
  });

  it('опасное изменение (смена URL) требует подтверждения', async () => {
    const saved = vi.fn();
    baseMocks({ unlocked: true });
    setInvoke('save_settings', (args) => {
      const a = args as { confirmDangerous: boolean };
      saved(a);
      return a.confirmDangerous
        ? { kind: 'saved' }
        : { kind: 'needs_confirmation', dangerous: ['Смена адреса сервера ex_system'] };
    });
    renderAdmin();

    const url = await screen.findByLabelText('URL сервера ex_system');
    fireEvent.change(url, { target: { value: 'https://other.example' } });
    fireEvent.click(screen.getByText('Сохранить'));

    // Появился диалог подтверждения опасных изменений.
    expect(await screen.findByText('Подтвердите опасные изменения')).toBeInTheDocument();
    expect(screen.getByText('Смена адреса сервера ex_system')).toBeInTheDocument();
    // Первый вызов — без подтверждения.
    expect(saved.mock.calls[0][0].confirmDangerous).toBe(false);

    fireEvent.click(screen.getByText('Подтвердить и сохранить'));
    // Повторный вызов — уже с подтверждением.
    await waitFor(() => expect(saved).toHaveBeenCalledTimes(2));
    expect(saved.mock.calls[1][0].confirmDangerous).toBe(true);
  });

  it('валидирует обязательный URL сервера при авто-выгрузке (после разблокировки)', async () => {
    baseMocks({ unlocked: true });
    setInvoke('save_settings', () => ({ kind: 'saved' }));
    renderAdmin();

    const url = await screen.findByLabelText('URL сервера ex_system');
    fireEvent.change(url, { target: { value: '' } });

    expect(
      await screen.findByText('URL сервера обязателен при авто-выгрузке'),
    ).toBeInTheDocument();
    expect(screen.getByText('Сохранить').closest('button')).toBeDisabled();
  });

  it('не задан админ-PIN — понятная подсказка, изменений нет', async () => {
    baseMocks({ unlocked: false, provisioned: false });
    renderAdmin();

    expect(
      await screen.findAllByText(/Админ-PIN не задан при развёртывании/),
    ).not.toHaveLength(0);
    expect(screen.queryByText('Разблокировать')).not.toBeInTheDocument();
    expect(screen.queryByText('Экспортировать профиль')).not.toBeInTheDocument();
  });
});
