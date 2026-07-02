import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { BlockHead, Button, Card, EmptyState, Skeleton, Tag } from '../design';
import {
  formatAdjudicationRef,
  listSessions,
  pauseUpload,
  resumeUpload,
  retryUpload,
  type SessionView,
  type UploadStatus,
} from '../lib/core';

// Экран «Сессии» (этап 04 + 06). Список локальных записей из манифеста (этап 03):
// дата, длительность, привязка к делу, статус и прогресс выгрузки в ex_system.
// Управление выгрузкой (этап 06): повтор/пауза/продолжение — команды ipc::sync_cmds.

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
    <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
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

      {load.kind === 'ready' &&
        load.sessions.map((s) => {
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
