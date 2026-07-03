// Поиск, фильтрация, сортировка и пагинация списка сессий (этап 10.6,
// deliverable 3). Чистые функции над уже загруженным `listSessions()` — без
// нового backend-запроса; полностью тестируемы. Экран «Сессии» их комбинирует.

import { formatAdjudicationRef, type SessionView, type UploadStatus } from './core';

/** Фильтр по статусу выгрузки: конкретный статус или «любой». */
export type UploadFilter = UploadStatus | 'all';

/** Направление сортировки по дате начала сессии. */
export type SortOrder = 'newest' | 'oldest';

export interface SessionQuery {
  /** Строка поиска по № дела / ФИО (регистронезависимо); пусто — без фильтра. */
  search: string;
  /** Фильтр по статусу выгрузки. */
  upload: UploadFilter;
  /** Сортировка по дате. */
  sort: SortOrder;
}

export const DEFAULT_QUERY: SessionQuery = {
  search: '',
  upload: 'all',
  sort: 'newest',
};

/**
 * Отфильтровать сессии по поисковой строке (№ дела/ФИО из `adjudication_ref`) и
 * статусу выгрузки. Записывающиеся/удалённые сессии учитывают только статус
 * выгрузки, если он явно выбран (иначе показываются как есть).
 */
export function filterSessions(sessions: SessionView[], q: SessionQuery): SessionView[] {
  const needle = q.search.trim().toLowerCase();
  return sessions.filter((s) => {
    if (needle) {
      const ref = formatAdjudicationRef(s.adjudication_ref) ?? '';
      if (!ref.toLowerCase().includes(needle)) return false;
    }
    if (q.upload !== 'all' && s.upload_status !== q.upload) return false;
    return true;
  });
}

/** Отсортировать сессии по дате начала (не мутирует вход). */
export function sortSessions(sessions: SessionView[], order: SortOrder): SessionView[] {
  const copy = [...sessions];
  copy.sort((a, b) =>
    order === 'newest'
      ? b.started_at_unix_ms - a.started_at_unix_ms
      : a.started_at_unix_ms - b.started_at_unix_ms,
  );
  return copy;
}

/** Результат постраничной нарезки: элементы страницы + метаданные для навигации. */
export interface Page<T> {
  items: T[];
  page: number;
  pageCount: number;
  total: number;
}

/**
 * Нарезать список на страницу `page` (0-based) по `pageSize`. `page` зажимается в
 * допустимый диапазон (пустой список → страница 0, одна пустая страница).
 */
export function paginate<T>(items: T[], page: number, pageSize: number): Page<T> {
  const size = Math.max(1, Math.floor(pageSize));
  const total = items.length;
  const pageCount = Math.max(1, Math.ceil(total / size));
  const clamped = Math.min(Math.max(0, Math.floor(page)), pageCount - 1);
  const start = clamped * size;
  return {
    items: items.slice(start, start + size),
    page: clamped,
    pageCount,
    total,
  };
}

/** Полный конвейер: фильтр → сортировка → пагинация (для экрана «Сессии»). */
export function querySessions(
  sessions: SessionView[],
  q: SessionQuery,
  page: number,
  pageSize: number,
): Page<SessionView> {
  return paginate(sortSessions(filterSessions(sessions, q), q.sort), page, pageSize);
}
