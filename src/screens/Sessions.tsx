import { useCallback, useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { BlockHead, Button, Card, EmptyState, Field, fieldCaptionStyle, NEUTRAL_BTN, Select, screenStackStyle, Skeleton, Tag } from '../design';
import {
  formatAdjudicationRef,
  listSessions,
  pauseUpload,
  resumeUpload,
  retryUpload,
  type SessionView,
  type UploadStatus,
} from '../lib/core';
import {
  querySessions,
  type SortOrder,
  type UploadFilter,
} from '../lib/session-filter';

// Экран «Сессии» (этап 04 + 06 + 10.6). Список локальных записей из манифеста
// (этап 03): дата, длительность, привязка к делу, статус и прогресс выгрузки.
// Поиск/фильтры/сортировка/пагинация (этап 10.6) — чистые хелперы `session-filter`
// поверх уже загруженного списка. Управление выгрузкой (этап 06) — ipc::sync_cmds;
// карточка сессии (10.6) — маршрут `/sessions/:dir`.

// Сколько записей на страницу (презентация, не бизнес-параметр реестра).
const PAGE_SIZE = 10;

// Человекочитаемые ярлыки фильтра по статусу выгрузки.
const UPLOAD_FILTER_LABELS: Record<UploadFilter, string> = {
  all: 'Любой статус',
  pending: 'Готова к выгрузке',
  uploading: 'Выгружается',
  uploaded: 'Выгружена',
  confirmed: 'Подтверждена',
  failed: 'Ошибка выгрузки',
  integrity_failed: 'Ошибка целостности',
};

const SORT_LABELS: Record<SortOrder, string> = {
  newest: 'Сначала новые',
  oldest: 'Сначала старые',
};

type Tone = 'default' | 'accent' | 'gold' | 'green';

interface StatusView {
  label: string;
  tone: Tone;
}

/** Доля выгруженных частей в процентах (для статуса «Выгружается N%»). */
function uploadPercent(s: SessionView): number {
  if (s.upload_total_parts === 0) return 0;
  return Math.floor((s.upload_sent_parts / s.upload_total_parts) * 100);
}

/** Единый статус строки: запись активна / удалена / пауза / иначе — по выгрузке. */
function deriveStatus(s: SessionView): StatusView {
  if (s.status === 'recording') return { label: 'Записывается', tone: 'accent' };
  if (s.status === 'purged') return { label: 'Удалена локально', tone: 'default' };
  if (s.upload_paused) return { label: 'Выгрузка на паузе', tone: 'default' };
  return uploadStatusView(s.upload_status, uploadPercent(s));
}

function uploadStatusView(upload: UploadStatus, percent: number): StatusView {
  switch (upload) {
    case 'pending':
      return { label: 'Готова к выгрузке', tone: 'default' };
    case 'uploading':
      return { label: `Выгружается ${percent}%`, tone: 'gold' };
    case 'uploaded':
      return { label: 'Выгружена', tone: 'gold' };
    case 'confirmed':
      return { label: 'Подтверждена', tone: 'green' };
    case 'failed':
      return { label: 'Ошибка выгрузки', tone: 'accent' };
    case 'integrity_failed':
      return { label: 'Ошибка целостности', tone: 'accent' };
  }
}

/** Какие действия выгрузки доступны для записи (этап 06). */
function uploadActions(s: SessionView): {
  canRetry: boolean;
  canPause: boolean;
  canResume: boolean;
} {
  const finished = s.status !== 'recording' && s.status !== 'purged';
  const done = s.upload_status === 'confirmed';
  return {
    canRetry: finished && !done && (s.upload_status === 'failed' || s.upload_status === 'integrity_failed'),
    canPause: finished && !done && !s.upload_paused,
    canResume: finished && !done && s.upload_paused,
  };
}

type Load =
  | { kind: 'loading' }
  | { kind: 'ready'; sessions: SessionView[] }
  | { kind: 'error'; message: string };

/** Доступно ли прослушивание (этап 10.1): завершённая, не удалённая локально сессия. */
function canListen(s: SessionView): boolean {
  return s.status !== 'recording' && s.status !== 'purged';
}

export function SessionsScreen() {
  const navigate = useNavigate();
  const [load, setLoad] = useState<Load>({ kind: 'loading' });
  const [busy, setBusy] = useState<string | null>(null);

  // Поиск/фильтры/сортировка/пагинация (этап 10.6) — состояние UI.
  const [search, setSearch] = useState('');
  const [upload, setUpload] = useState<UploadFilter>('all');
  const [sort, setSort] = useState<SortOrder>('newest');
  const [page, setPage] = useState(0);

  const reload = useCallback(() => {
    let active = true;
    listSessions()
      .then((sessions) => active && setLoad({ kind: 'ready', sessions }))
      .catch((e: unknown) => active && setLoad({ kind: 'error', message: describeError(e) }));
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => reload(), [reload]);

  // Смена критериев сбрасывает на первую страницу (иначе можно «зависнуть» на
  // несуществующей странице после сужения выборки).
  useEffect(() => setPage(0), [search, upload, sort]);

  const allSessions = load.kind === 'ready' ? load.sessions : [];
  const result = useMemo(
    () => querySessions(allSessions, { search, upload, sort }, page, PAGE_SIZE),
    [allSessions, search, upload, sort, page],
  );

  const runAction = useCallback(
    async (id: string, action: () => Promise<void>) => {
      setBusy(id);
      try {
        await action();
        reload();
      } catch (e: unknown) {
        setLoad({ kind: 'error', message: describeError(e) });
      } finally {
        setBusy(null);
      }
    },
    [reload],
  );

  return (
    <div style={screenStackStyle(880)}>
      <Card>
        <BlockHead
          numeral="02"
          title="Сессии"
          hint="История записей станции и статус их выгрузки в ex_system"
        />
        {load.kind === 'error' && (
          <div style={{ marginTop: 8 }}>
            <Tag tone="accent">Ошибка: {load.message}</Tag>
          </div>
        )}
        {/* Поиск, фильтр и сортировка (этап 10.6). */}
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(200px, 1fr))',
            gap: 12,
            marginTop: 16,
          }}
        >
          <Field
            label="Поиск по № дела / ФИО"
            value={search}
            placeholder="например: 1-100 или Иванов"
            onChange={(e) => setSearch(e.target.value)}
          />
          <label style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            <span style={fieldCaptionStyle}>Статус выгрузки</span>
            <Select
              ariaLabel="Фильтр по статусу выгрузки"
              value={upload}
              onChange={(v) => setUpload(v as UploadFilter)}
              options={(Object.keys(UPLOAD_FILTER_LABELS) as UploadFilter[]).map((k) => ({
                value: k,
                label: UPLOAD_FILTER_LABELS[k],
              }))}
            />
          </label>
          <label style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            <span style={fieldCaptionStyle}>Сортировка</span>
            <Select
              ariaLabel="Сортировка сессий"
              value={sort}
              onChange={(v) => setSort(v as SortOrder)}
              options={(Object.keys(SORT_LABELS) as SortOrder[]).map((k) => ({
                value: k,
                label: SORT_LABELS[k],
              }))}
            />
          </label>
        </div>
      </Card>

      {load.kind === 'loading' && (
        <Card>
          <Skeleton variant="block" count={3} height={48} />
        </Card>
      )}

      {load.kind === 'ready' && load.sessions.length === 0 && (
        <EmptyState
          icon="icon-step-case"
          title="Записанных сессий пока нет"
          description="Завершённые записи появятся здесь с длительностью, привязкой к делу и статусом выгрузки в ex_system."
        />
      )}

      {load.kind === 'ready' && load.sessions.length > 0 && result.items.length === 0 && (
        <EmptyState
          icon="icon-step-case"
          title="Ничего не найдено"
          description="Под текущий поиск/фильтр записей нет. Измените запрос или сбросьте фильтр."
        />
      )}

      {load.kind === 'ready' &&
        result.items.map((s) => {
          const sv = deriveStatus(s);
          const actions = uploadActions(s);
          const isBusy = busy === s.dir;
          return (
            <Card key={s.id}>
              <div
                style={{
                  display: 'flex',
                  alignItems: 'flex-start',
                  gap: 16,
                  flexWrap: 'wrap',
                }}
              >
                <div style={{ flex: 1, minWidth: 220 }}>
                  <div
                    style={{
                      fontFamily: 'var(--serif)',
                      fontSize: 16,
                      fontWeight: 500,
                      color: 'var(--ink)',
                    }}
                  >
                    {formatAdjudicationRef(s.adjudication_ref) ?? 'Без привязки к делу'}
                  </div>
                  <div
                    className="num"
                    style={{ fontSize: 12, color: 'var(--ink-soft)', marginTop: 4 }}
                  >
                    {formatDateTime(s.started_at_unix_ms)} · {formatDuration(s.duration_seconds)} ·{' '}
                    {s.segment_count} сегм. · {formatHz(s.sample_rate_hz)} / {s.bit_depth} бит
                  </div>
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
                  <Button
                    variant="mini"
                    onClick={() => navigate(`/sessions/${encodeURIComponent(s.dir)}`)}
                  >
                    Карточка
                  </Button>
                  {canListen(s) && (
                    <Button
                      variant="mini"
                      onClick={() => navigate(`/sessions/${encodeURIComponent(s.dir)}/listen`)}
                    >
                      Прослушать
                    </Button>
                  )}
                  {canListen(s) && (
                    <Button
                      variant="mini"
                      onClick={() => navigate(`/sessions/${encodeURIComponent(s.dir)}/export`)}
                    >
                      Экспортировать
                    </Button>
                  )}
                  {actions.canRetry && (
                    <Button
                      variant="mini"
                      loading={isBusy}
                      onClick={() => runAction(s.dir, () => retryUpload(s.dir))}
                    >
                      Повторить
                    </Button>
                  )}
                  {actions.canPause && (
                    <Button
                      variant="mini"
                      loading={isBusy}
                      onClick={() => runAction(s.dir, () => pauseUpload(s.dir))}
                    >
                      Пауза
                    </Button>
                  )}
                  {actions.canResume && (
                    <Button
                      variant="mini"
                      loading={isBusy}
                      onClick={() => runAction(s.dir, () => resumeUpload(s.dir))}
                    >
                      Продолжить
                    </Button>
                  )}
                  <Tag tone={sv.tone}>{sv.label}</Tag>
                </div>
              </div>
            </Card>
          );
        })}

      {/* Пагинация (этап 10.6): показываем, только если страниц больше одной. */}
      {load.kind === 'ready' && result.pageCount > 1 && (
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            gap: 12,
            flexWrap: 'wrap',
          }}
        >
          <Button
            variant="secondary"
            style={NEUTRAL_BTN}
            disabled={result.page === 0}
            onClick={() => setPage((p) => Math.max(0, p - 1))}
          >
            ← Назад
          </Button>
          <span className="num" style={{ fontSize: 13, color: 'var(--ink-soft)' }}>
            Страница {result.page + 1} из {result.pageCount} · всего {result.total}
          </span>
          <Button
            variant="secondary"
            style={NEUTRAL_BTN}
            disabled={result.page >= result.pageCount - 1}
            onClick={() => setPage((p) => Math.min(result.pageCount - 1, p + 1))}
          >
            Вперёд →
          </Button>
        </div>
      )}
    </div>
  );
}

function formatDateTime(unixMs: number): string {
  return new Date(unixMs).toLocaleString('ru-RU');
}

function formatDuration(totalSec: number): string {
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(h)}:${pad(m)}:${pad(s)}`;
}

function formatHz(hz: number): string {
  return `${(hz / 1000).toFixed(1).replace(/\.0$/, '')} кГц`;
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}
