import type { ReactNode } from 'react';
import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { BlockHead, Button, Card, InfoTip, NEUTRAL_BTN, Skeleton, Tag } from '../design';
import { SelfTestPanel } from '../components/SelfTest';
import {
  getDiagnostics,
  type DiagnosticsInfo,
  type DiskStatusCode,
  type EventKind,
} from '../lib/core';

// Экран «Диагностика» (этап 04). Реальное состояние станции: здоровье
// устройства, свободное место, последние события записи/восстановления, статус
// целостности последней сессии, версия/идентичность станции. Только чтение
// (`diagnostics`) — без дублирования логики ядра.

const DISK_TONE: Record<DiskStatusCode, 'green' | 'gold' | 'accent'> = {
  ok: 'green',
  low: 'gold',
  critical: 'accent',
};

const DISK_LABEL: Record<DiskStatusCode, string> = {
  ok: 'Достаточно',
  low: 'Мало места',
  critical: 'Критически мало',
};

const EVENT_LABEL: Record<EventKind, string> = {
  session_started: 'Старт сессии',
  segment_rotated: 'Ротация сегмента',
  paused: 'Пауза',
  resumed: 'Возобновление',
  device_lost: 'Обрыв устройства',
  device_back: 'Возврат устройства',
  recovered: 'Восстановление',
  stopped: 'Завершение',
  playback_accessed: 'Доступ к прослушиванию',
};

type Load =
  | { kind: 'loading' }
  | { kind: 'ready'; data: DiagnosticsInfo }
  | { kind: 'error'; message: string };

export function DiagnosticsScreen() {
  const navigate = useNavigate();
  const [load, setLoad] = useState<Load>({ kind: 'loading' });

  useEffect(() => {
    let active = true;
    getDiagnostics()
      .then((data) => active && setLoad({ kind: 'ready', data }))
      .catch((e: unknown) => active && setLoad({ kind: 'error', message: describeError(e) }));
    return () => {
      active = false;
    };
  }, []);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
      <Card>
        <BlockHead
          numeral="04"
          title="Диагностика"
          hint="Состояние устройства, диска, событий записи и целостности"
        />
        {load.kind === 'error' && (
          <div style={{ marginTop: 8 }}>
            <Tag tone="accent">Ошибка: {load.message}</Tag>
          </div>
        )}
        <div style={{ marginTop: 12 }}>
          <Button variant="secondary" style={NEUTRAL_BTN} onClick={() => navigate('/setup')}>
            Мастер первого запуска
          </Button>
        </div>
      </Card>

      {/* Проверка перед заседанием (этап 10.6) — доступна и с «Диагностики». */}
      <SelfTestPanel numeral="✓" />

      {load.kind === 'loading' && (
        <Card>
          <Skeleton variant="block" count={4} height={40} />
        </Card>
      )}

      {load.kind === 'ready' && <DiagnosticsBody data={load.data} />}
    </div>
  );
}

function DiagnosticsBody({ data }: { data: DiagnosticsInfo }) {
  const { devices, disk, station, recent_events, integrity } = data;
  return (
    <>
      <Card>
        <BlockHead numeral="A" title="Устройства ввода" />
        {devices.length === 0 ? (
          <Tag tone="accent">Устройства не обнаружены</Tag>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginTop: 8 }}>
            {devices.map((d) => (
              <div
                key={d.name}
                style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}
              >
                <span style={{ color: 'var(--ink)', fontWeight: 500 }}>{d.name}</span>
                {d.is_default && <Tag tone="green">по умолчанию</Tag>}
                <span className="num" style={{ fontSize: 12, color: 'var(--muted)' }}>
                  {d.default_sample_rate_hz ? formatHz(d.default_sample_rate_hz) : '—'} ·{' '}
                  {d.default_channels ?? '?'} кан.
                </span>
              </div>
            ))}
          </div>
        )}
      </Card>

      <Card>
        <BlockHead numeral="B" title="Свободное место" />
        <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 8 }}>
          <span className="num" style={{ fontSize: 22, color: 'var(--ink)' }}>
            {disk.free_mb.toLocaleString('ru-RU')} МБ
          </span>
          <Tag tone={DISK_TONE[disk.status]}>{DISK_LABEL[disk.status]}</Tag>
          <span style={{ fontSize: 12, color: 'var(--muted)' }}>
            пороги: {disk.low_threshold_mb} / {disk.critical_mb} МБ
          </span>
        </div>
      </Card>

      <Card>
        <BlockHead numeral="C" title="Целостность последней сессии" />
        {integrity ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 6, marginTop: 8 }}>
            <Row label="Сессия">
              <span className="num">{integrity.session_id}</span>
            </Row>
            <Row label="Сегменты с хешем">
              <span className="num">
                {integrity.segments_hashed} / {integrity.segments}
              </span>
              {integrity.segments_hashed === integrity.segments && integrity.segments > 0 && (
                <Tag tone="green">все хешированы</Tag>
              )}
            </Row>
            <Row label="Хеш-цепочка">
              {integrity.hash_chain_enabled ? (
                <Tag tone="green">включена</Tag>
              ) : (
                <Tag tone="gold">выключена</Tag>
              )}
              <InfoTip text="Финальное звено фиксирует целостность всей цепочки сегментов." />
            </Row>
            <Row label="Финальное звено">
              <span className="num" style={{ fontSize: 12, wordBreak: 'break-all' }}>
                {integrity.final_chain_link ?? '—'}
              </span>
            </Row>
            <Row label="Журнал событий">
              {integrity.event_log_enabled ? (
                <Tag tone="green">включён</Tag>
              ) : (
                <Tag tone="gold">выключен</Tag>
              )}
            </Row>
          </div>
        ) : (
          <p style={{ color: 'var(--muted)', marginTop: 8 }}>Сессий пока нет.</p>
        )}
      </Card>

      <Card>
        <BlockHead numeral="D" title="Последние события записи" />
        {recent_events.length === 0 ? (
          <p style={{ color: 'var(--muted)', marginTop: 8 }}>Событий пока нет.</p>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4, marginTop: 8 }}>
            {recent_events.map((ev) => (
              <div key={ev.seq} style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                <span
                  className="num"
                  style={{ fontSize: 12, color: 'var(--muted)', minWidth: 150 }}
                >
                  {formatDateTime(ev.at_unix_ms)}
                </span>
                <span style={{ color: 'var(--ink-soft)' }}>{EVENT_LABEL[ev.kind]}</span>
              </div>
            ))}
          </div>
        )}
      </Card>

      <Card>
        <BlockHead numeral="E" title="Станция" />
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, marginTop: 8 }}>
          <Row label="Версия">
            <span className="num">{station.app_version}</span>
          </Row>
          <Row label="Идентификатор">
            <span className="num">{station.station_id ?? 'не настроена'}</span>
          </Row>
          <Row label="Хранилище">
            <span className="num" style={{ fontSize: 12, wordBreak: 'break-all' }}>
              {station.storage_root}
            </span>
          </Row>
        </div>
      </Card>
    </>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
      <span style={{ fontSize: 12, color: 'var(--muted)', minWidth: 160 }}>{label}</span>
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8, color: 'var(--ink)' }}>
        {children}
      </span>
    </div>
  );
}

function formatDateTime(unixMs: number): string {
  return new Date(unixMs).toLocaleString('ru-RU');
}

function formatHz(hz: number): string {
  return `${(hz / 1000).toFixed(1).replace(/\.0$/, '')} кГц`;
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}
