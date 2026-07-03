import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { SelfTestPanel } from './SelfTest';
import { setInvoke } from '../test/tauriMock';
import type { SelfTestReport } from '../lib/core';

// Self-test панель (этап 10.6): мокаем команду ядра `self_test` и проверяем, что
// панель рисует чек-лист, агрегат «можно начинать» и кнопки «Исправить» для
// не-ok позиций.

function mockReport(report: SelfTestReport) {
  setInvoke('self_test', () => report);
}

function renderPanel() {
  return render(
    <MemoryRouter>
      <SelfTestPanel />
    </MemoryRouter>,
  );
}

describe('SelfTestPanel', () => {
  it('показывает «Можно начинать» когда нет ни одного fail', async () => {
    mockReport({
      ready: true,
      checks: [
        { id: 'device', label: 'Устройство ввода', status: 'ok', detail: 'Найдено: 1.' },
        { id: 'disk', label: 'Свободное место на диске', status: 'warn', detail: 'Мало места.' },
      ],
    });
    renderPanel();
    expect(await screen.findByText('✓ Можно начинать')).toBeInTheDocument();
    expect(screen.getByText('Устройство ввода')).toBeInTheDocument();
    expect(screen.getByText('Свободное место на диске')).toBeInTheDocument();
  });

  it('показывает список проблем и кнопку «Исправить» при fail', async () => {
    mockReport({
      ready: false,
      checks: [
        {
          id: 'operator',
          label: 'Вход оператора',
          status: 'fail',
          detail: 'Оператор не вошёл.',
          fix: 'open_login',
        },
      ],
    });
    renderPanel();
    expect(await screen.findByText('Есть проблемы — исправьте отмеченное')).toBeInTheDocument();
    // Для fail-позиции с fix есть кнопка навигации.
    expect(screen.getByRole('button', { name: 'Ко входу' })).toBeInTheDocument();
  });

  it('не показывает кнопку «Исправить» у ok-позиций', async () => {
    mockReport({
      ready: true,
      checks: [{ id: 'device', label: 'Устройство ввода', status: 'ok', detail: 'ок' }],
    });
    renderPanel();
    await screen.findByText('✓ Можно начинать');
    expect(screen.queryByRole('button', { name: /К настройкам|К записи|Ко входу/ })).toBeNull();
  });

  it('перезапускает проверку по кнопке «Проверить снова»', async () => {
    let calls = 0;
    setInvoke('self_test', () => {
      calls += 1;
      return {
        ready: true,
        checks: [{ id: 'device', label: 'Устройство ввода', status: 'ok', detail: 'ок' }],
      } satisfies SelfTestReport;
    });
    renderPanel();
    await screen.findByText('✓ Можно начинать');
    expect(calls).toBe(1);
    await userEvent.click(screen.getByRole('button', { name: 'Проверить снова' }));
    await waitFor(() => expect(calls).toBe(2));
  });
});
