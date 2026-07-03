import { describe, expect, it } from 'vitest';
import {
  DEFAULT_QUERY,
  filterSessions,
  paginate,
  querySessions,
  sortSessions,
} from './session-filter';
import { sessionViewFixture } from '../test/fixtures';
import type { SessionView } from './core';

// Чистые хелперы поиска/сортировки/пагинации сессий (этап 10.6).

function sessions(): SessionView[] {
  return [
    sessionViewFixture({
      id: 's1',
      dir: '/rec/s1',
      started_at_unix_ms: 1_000,
      adjudication_ref: '№ 1-100/2026, Иванов И.И.',
      upload_status: 'confirmed',
    }),
    sessionViewFixture({
      id: 's2',
      dir: '/rec/s2',
      started_at_unix_ms: 3_000,
      adjudication_ref: '№ 2-200/2026, Петров П.П.',
      upload_status: 'failed',
    }),
    sessionViewFixture({
      id: 's3',
      dir: '/rec/s3',
      started_at_unix_ms: 2_000,
      adjudication_ref: null,
      upload_status: 'pending',
    }),
  ];
}

describe('filterSessions', () => {
  it('ищет по № дела/ФИО регистронезависимо', () => {
    const out = filterSessions(sessions(), { ...DEFAULT_QUERY, search: 'петров' });
    expect(out.map((s) => s.id)).toEqual(['s2']);
  });

  it('находит по номеру дела', () => {
    const out = filterSessions(sessions(), { ...DEFAULT_QUERY, search: '1-100' });
    expect(out.map((s) => s.id)).toEqual(['s1']);
  });

  it('без привязки не попадает под непустой поиск', () => {
    const out = filterSessions(sessions(), { ...DEFAULT_QUERY, search: 'иванов' });
    expect(out.map((s) => s.id)).toEqual(['s1']);
  });

  it('фильтрует по статусу выгрузки', () => {
    const out = filterSessions(sessions(), { ...DEFAULT_QUERY, upload: 'failed' });
    expect(out.map((s) => s.id)).toEqual(['s2']);
  });

  it('пустой запрос возвращает все', () => {
    expect(filterSessions(sessions(), DEFAULT_QUERY)).toHaveLength(3);
  });
});

describe('sortSessions', () => {
  it('newest — новые сверху', () => {
    expect(sortSessions(sessions(), 'newest').map((s) => s.id)).toEqual(['s2', 's3', 's1']);
  });
  it('oldest — старые сверху', () => {
    expect(sortSessions(sessions(), 'oldest').map((s) => s.id)).toEqual(['s1', 's3', 's2']);
  });
  it('не мутирует вход', () => {
    const input = sessions();
    sortSessions(input, 'oldest');
    expect(input[0].id).toBe('s1');
  });
});

describe('paginate', () => {
  const items = [1, 2, 3, 4, 5];
  it('нарезает по размеру страницы', () => {
    const p = paginate(items, 0, 2);
    expect(p.items).toEqual([1, 2]);
    expect(p.pageCount).toBe(3);
    expect(p.total).toBe(5);
  });
  it('зажимает страницу за пределами диапазона', () => {
    const p = paginate(items, 99, 2);
    expect(p.page).toBe(2);
    expect(p.items).toEqual([5]);
  });
  it('пустой список — одна пустая страница', () => {
    const p = paginate([], 0, 10);
    expect(p.items).toEqual([]);
    expect(p.pageCount).toBe(1);
    expect(p.total).toBe(0);
  });
});

describe('querySessions', () => {
  it('фильтр + сортировка + пагинация вместе', () => {
    const p = querySessions(sessions(), { ...DEFAULT_QUERY, sort: 'oldest' }, 0, 2);
    expect(p.items.map((s) => s.id)).toEqual(['s1', 's3']);
    expect(p.pageCount).toBe(2);
  });
});
