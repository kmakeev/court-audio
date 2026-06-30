import { useEffect, useState } from 'react';
import { BlockHead, Card, EmptyState, Skeleton, Tag } from '../design';
import {
  listSessions,
  type SessionStatus,
  type SessionView,
  type UploadStatus,
} from '../lib/core';

// Экран «Сессии» (этап 04). Список локальных записей из манифеста (этап 03):
// дата, длительность, привязка к делу, статус обработки/выгрузки. Только чтение
// (`list_sessions`); выгрузка и прогресс — этап 06.

type Tone = 'default' | 'accent' | 'gold' | 'green';

interface StatusView {
  label: string;
  tone: Tone;
}

/** Единый статус строки: запись активна / удалена / иначе — по статусу выгрузки. */
function deriveStatus(status: SessionStatus, upload: UploadStatus): StatusView {
  if (status === 'recording') return { label: 'Записывается', tone: 'accent' };
  if (status === 'purged') return { label: 'Удалена локально', tone: 'default' };
  switch (upload) {
    case 'pending':
      return { label: 'Готова к выгрузке', tone: 'default' };
    case 'uploading':
      return { label: 'Выгружается', tone: 'gold' };
    case 'uploaded':
      return { label: 'Выгружена', tone: 'gold' };
    case 'confirmed':
      return { label: 'Подтверждена', tone: 'green' };
    case 'failed':
      return { label: 'Ошибка выгрузки', tone: 'accent' };
  }
}

type Load =
  | { kind: 'loading' }
  | { kind: 'ready'; sessions: SessionView[] }
  | { kind: 'error'; message: string };

export function SessionsScreen() {
  const [load, setLoad] = useState<Load>({ kind: 'loading' });

  useEffect(() => {
    let active = true;
    listSessions()
      .then((sessions) => active && setLoad({ kind: 'ready', sessions }))
      .catch((e: unknown) => active && setLoad({ kind: 'error', message: describeError(e) }));
    return () => {
      active = false;
    };
  }, []);

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
          const sv = deriveStatus(s.status, s.upload_status);
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
                    {s.adjudication_ref ?? 'Без привязки к делу'}
                  </div>
                  <div
                    className="num"
                    style={{ fontSize: 12, color: 'var(--ink-soft)', marginTop: 4 }}
                  >
                    {formatDateTime(s.started_at_unix_ms)} · {formatDuration(s.duration_seconds)} ·{' '}
                    {s.segment_count} сегм. · {formatHz(s.sample_rate_hz)} / {s.bit_depth} бит
                  </div>
                </div>
                <Tag tone={sv.tone}>{sv.label}</Tag>
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
