import { describe, it, expect, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { CasePicker } from './CasePicker';
import { setInvoke } from '../test/tauriMock';
import type { AdjudicationRef, CaseCacheStatus, CaseRecord } from '../lib/core';

const CASES: CaseRecord[] = [
  { id: 'adj-1', number: '№ 1-123/2026', fio: 'Иванов Иван Иванович', date: '2026-06-30' },
  { id: 'adj-2', number: '№ 2-7/2026', fio: 'Петрова Анна Сергеевна', date: '2026-06-28' },
];

function wireCache(status: Partial<CaseCacheStatus> = {}, cases: CaseRecord[] = CASES) {
  setInvoke('get_case_cache_status', () => ({
    synced_at_unix_ms: Date.now(),
    is_fresh: true,
    record_count: cases.length,
    scope: 'court_docket',
    ...status,
  }));
  setInvoke('search_cases', (args) => {
    const q = String((args as { query?: string })?.query ?? '').toLowerCase();
    if (!q) return cases;
    return cases.filter(
      (c) => c.number.toLowerCase().includes(q) || c.fio.toLowerCase().includes(q),
    );
  });
  setInvoke('sync_case_cache', () => {
    throw 'синхронизация появится в этапе 06/07';
  });
}

/** Обёртка-хост: держит привязку как состояние, чтобы наблюдать onChange. */
function Host({ onBinding }: { onBinding?: (b: AdjudicationRef | null) => void }) {
  const [binding, setBinding] = useState<AdjudicationRef | null>(null);
  return (
    <CasePicker
      binding={binding}
      onChange={(b) => {
        setBinding(b);
        onBinding?.(b);
      }}
    />
  );
}

describe('CasePicker', () => {
  it('автокомплит по кэшу выдаёт resolved-привязку при выборе дела', async () => {
    wireCache();
    const onBinding = vi.fn();
    render(<Host onBinding={onBinding} />);

    const input = await screen.findByLabelText('Поиск дела (№ или ФИО)');
    act(() => {
      fireEvent.focus(input);
      fireEvent.change(input, { target: { value: 'Петров' } });
    });

    const option = await screen.findByText('№ 2-7/2026');
    fireEvent.click(option);

    await waitFor(() =>
      expect(onBinding).toHaveBeenLastCalledWith({
        kind: 'resolved',
        adjudication_id: 'adj-2',
        raw_number: '№ 2-7/2026',
        raw_fio: 'Петрова Анна Сергеевна',
      }),
    );
    expect(await screen.findByText('✓ Дело выбрано')).toBeInTheDocument();
  });

  it('ручной ввод даёт pending-привязку (manual)', async () => {
    wireCache();
    const onBinding = vi.fn();
    render(<Host onBinding={onBinding} />);

    fireEvent.click(await screen.findByRole('checkbox'));
    const numberField = await screen.findByLabelText('№ дела');
    act(() => {
      fireEvent.change(numberField, { target: { value: '№ 9-1/2026' } });
    });

    await waitFor(() =>
      expect(onBinding).toHaveBeenLastCalledWith({
        kind: 'manual',
        raw_number: '№ 9-1/2026',
        raw_fio: undefined,
      }),
    );
    expect(
      await screen.findByText('⚠ Ручной ввод — связывание на сервере'),
    ).toBeInTheDocument();
  });

  it('показывает свежесть кэша: актуален / устарел', async () => {
    wireCache({ is_fresh: true });
    const { unmount } = render(<Host />);
    expect(await screen.findByText(/Кэш актуален/)).toBeInTheDocument();
    unmount();

    wireCache({ is_fresh: false });
    render(<Host />);
    expect(await screen.findByText(/Кэш устарел/)).toBeInTheDocument();
  });

  it('оффлайн без кэша: предлагает ручной ввод и даёт pending', async () => {
    wireCache({ synced_at_unix_ms: null, is_fresh: false, record_count: 0 }, []);
    const onBinding = vi.fn();
    render(<Host onBinding={onBinding} />);

    // Кэш пуст → автоматически режим ручного ввода.
    const numberField = await screen.findByLabelText('№ дела');
    expect(await screen.findByText('Кэш дел не загружен')).toBeInTheDocument();
    act(() => {
      fireEvent.change(numberField, { target: { value: '№ 5-5/2026' } });
    });
    await waitFor(() =>
      expect(onBinding).toHaveBeenLastCalledWith({
        kind: 'manual',
        raw_number: '№ 5-5/2026',
        raw_fio: undefined,
      }),
    );
  });

  it('обновление кэша в этапе 05 сообщает о недоступности транспорта', async () => {
    wireCache();
    render(<Host />);
    fireEvent.click(await screen.findByLabelText('Обновить кэш дел'));
    expect(
      await screen.findByText(/синхронизация появится в этапе 06\/07/),
    ).toBeInTheDocument();
  });
});
