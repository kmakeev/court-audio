import { useCallback, useEffect, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import {
  BlockHead,
  Button,
  Card,
  NEUTRAL_BTN,
  screenStackStyle,
  Skeleton,
  Tag,
} from '../design';
import { formatClock } from '../lib/format';
import {
  formatAdjudicationRef,
  getSessionDetail,
  setSessionComment,
  type EventKind,
  type SessionDetail,
} from '../lib/core';

// Карточка сессии (этап 10.6, deliverable 3). Read-only детали конкретной записи:
// метки/интервалы ролей, журнал событий, статус целостности; действия
// «Прослушать» (10.1) / «Экспортировать» (10.2); свободный комментарий оператора
// и копирование пути/№ дела. Данные — команда `session_detail` (без аудит-побочек,
// в отличие от открытия в плеере).

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
  | { kind: 'ready'; detail: SessionDetail }
  | { kind: 'error'; message: string };

/** Можно ли прослушать/экспортировать: завершённая, не удалённая локально сессия. */
function isPlayable(d: SessionDetail): boolean {
  return d.status !== 'recording' && d.status !== 'purged';
}

export function SessionCardScreen() {
  const navigate = useNavigate();
  const params = useParams();
  const dir = params.dir ? decodeURIComponent(params.dir) : '';

  const [load, setLoad] = useState<Load>({ kind: 'loading' });

  const reload = useCallback(() => {
    if (!dir) {
      setLoad({ kind: 'error', message: 'не указан каталог сессии' });
      return;
    }
    let active = true;
    getSessionDetail(dir)
      .then((detail) => active && setLoad({ kind: 'ready', detail }))
      .catch((e: unknown) => active && setLoad({ kind: 'error', message: describeError(e) }));
    return () => {
      active = false;
    };
  }, [dir]);

  useEffect(() => reload(), [reload]);

  // Кнопка возврата — сверху и снизу списка блоков (как в проигрывателе), чтобы не
  // прокручивать длинную карточку до конца ради выхода.
  const backButton = (
    <Button variant="secondary" style={NEUTRAL_BTN} onClick={() => navigate('/sessions')}>
      ← К списку
    </Button>
  );

  return (
    <div style={screenStackStyle(880)}>
      <div>{backButton}</div>

      <Card>
        <BlockHead
          numeral="02"
          title="Карточка сессии"
          hint="Метки, журнал событий, целостность; прослушивание и экспорт записи"
        />
        {load.kind === 'error' && (
          <div style={{ marginTop: 8 }}>
            <Tag tone="accent">Ошибка: {load.message}</Tag>
          </div>
        )}
      </Card>

      {load.kind === 'loading' && (
        <Card>
          <Skeleton variant="block" count={4} height={40} />
        </Card>
      )}

      {load.kind === 'ready' && (
        <CardBody detail={load.detail} dir={dir} onCommentSaved={reload} />
      )}

      <div>{backButton}</div>
    </div>
  );
}

function CardBody({
  detail,
  dir,
  onCommentSaved,
}: {
  detail: SessionDetail;
  dir: string;
  onCommentSaved: () => void;
}) {
  const navigate = useNavigate();
  const playable = isPlayable(detail);
  const ref = formatAdjudicationRef(detail.adjudication_ref);

  return (
    <>
      <Card>
        <div style={{ display: 'flex', alignItems: 'flex-start', gap: 16, flexWrap: 'wrap' }}>
          <div style={{ flex: 1, minWidth: 240 }}>
            <div style={{ fontFamily: 'var(--serif)', fontSize: 18, fontWeight: 500, color: 'var(--ink)' }}>
              {ref ?? 'Без привязки к делу'}
            </div>
            <div className="num" style={{ fontSize: 12, color: 'var(--ink-soft)', marginTop: 6 }}>
              {new Date(detail.started_at_unix_ms).toLocaleString('ru-RU')} ·{' '}
              {formatClock(detail.duration_seconds)} · {detail.segment_count} сегм. ·{' '}
              {detail.sample_rate_hz / 1000} кГц / {detail.bit_depth} бит
            </div>
          </div>
          <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
            {playable && (
              <Button
                variant="mini"
                onClick={() => navigate(`/sessions/${encodeURIComponent(dir)}/listen`)}
              >
                Прослушать
              </Button>
            )}
            {playable && (
              <Button
                variant="mini"
                onClick={() => navigate(`/sessions/${encodeURIComponent(dir)}/export`)}
              >
                Экспортировать
              </Button>
            )}
          </div>
        </div>

        {/* Копирование пути и № дела (мелочи трения, этап 10.6). */}
        <div style={{ display: 'flex', gap: 8, marginTop: 14, flexWrap: 'wrap' }}>
          <CopyButton label="Копировать путь" value={detail.dir} />
          {ref && <CopyButton label="Копировать № дела" value={ref} />}
        </div>
      </Card>

      <IntegrityCard detail={detail} />

      <CommentCard dir={dir} initial={detail.comment} onSaved={onCommentSaved} />

      <MarkersCard detail={detail} />

      <EventsCard detail={detail} />
    </>
  );
}

function IntegrityCard({ detail }: { detail: SessionDetail }) {
  const i = detail.integrity;
  const allHashed = i.segments > 0 && i.segments_hashed === i.segments;
  return (
    <Card>
      <BlockHead numeral="C" title="Целостность" />
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginTop: 8, flexWrap: 'wrap' }}>
        <span className="num" style={{ fontSize: 14, color: 'var(--ink)' }}>
          Сегменты с хешем: {i.segments_hashed} / {i.segments}
        </span>
        {allHashed ? <Tag tone="green">все хешированы</Tag> : <Tag tone="gold">не завершено</Tag>}
        {i.hash_chain_enabled && <Tag tone="green">хеш-цепочка</Tag>}
      </div>
      <div className="num" style={{ fontSize: 12, color: 'var(--ink-soft)', marginTop: 8, wordBreak: 'break-all' }}>
        Финальное звено: {i.final_chain_link ?? '—'}
      </div>
    </Card>
  );
}

function CommentCard({
  dir,
  initial,
  onSaved,
}: {
  dir: string;
  initial: string | null;
  onSaved: () => void;
}) {
  const [text, setText] = useState(initial ?? '');
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save() {
    setSaving(true);
    setSaved(false);
    setError(null);
    try {
      await setSessionComment(dir, text);
      setSaved(true);
      onSaved();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card>
      <BlockHead
        numeral="D"
        title="Комментарий к сессии"
        hint="Свободная заметка оператора (локальная): напр. особенности заседания"
      />
      <textarea
        aria-label="Комментарий к сессии"
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setSaved(false);
        }}
        placeholder="например: свидетель опоздал на 20 минут"
        style={{
          width: '100%',
          minHeight: 80,
          marginTop: 12,
          padding: '8px 10px',
          border: '1px solid var(--hairline)',
          background: 'var(--paper)',
          color: 'var(--ink)',
          fontSize: 13,
          resize: 'vertical',
          boxSizing: 'border-box',
        }}
      />
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 8 }}>
        <Button variant="primary" loading={saving} onClick={() => void save()}>
          Сохранить комментарий
        </Button>
        {saved && <Tag tone="green">Сохранено</Tag>}
        {error && <Tag tone="accent">{error}</Tag>}
      </div>
    </Card>
  );
}

function MarkersCard({ detail }: { detail: SessionDetail }) {
  return (
    <Card>
      <BlockHead numeral="E" title={`Метки и роли (${detail.markers.length})`} />
      {detail.markers.length === 0 && detail.role_spans.length === 0 ? (
        <p style={{ fontSize: 13, color: 'var(--muted)', margin: '8px 0 0' }}>
          Разметки у этой сессии нет.
        </p>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginTop: 10 }}>
          {detail.markers.map((m) => (
            <div key={m.id} style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
              <span className="num" style={{ fontSize: 12, color: 'var(--ink)', width: 72 }}>
                {formatClock(Math.floor(m.offset_ms / 1000))}
              </span>
              <Tag tone="default">{m.category}</Tag>
              {m.comment && (
                <span style={{ fontSize: 12, color: 'var(--ink-soft)' }}>{m.comment}</span>
              )}
            </div>
          ))}
          {detail.role_spans.map((r) => (
            <div key={r.id} style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <span className="num" style={{ fontSize: 12, color: 'var(--ink)', width: 72 }}>
                {formatClock(Math.floor(r.start_offset_ms / 1000))}
              </span>
              <Tag tone="accent">{r.role}</Tag>
              <span className="num" style={{ fontSize: 12, color: 'var(--ink-soft)' }}>
                {r.end_offset_ms != null ? `до ${formatClock(Math.floor(r.end_offset_ms / 1000))}` : 'открыт'}
              </span>
            </div>
          ))}
        </div>
      )}
    </Card>
  );
}

function EventsCard({ detail }: { detail: SessionDetail }) {
  return (
    <Card>
      <BlockHead numeral="F" title="Журнал событий" />
      {detail.events.length === 0 ? (
        <p style={{ fontSize: 13, color: 'var(--muted)', margin: '8px 0 0' }}>Событий пока нет.</p>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 4, marginTop: 10 }}>
          {detail.events.map((ev) => (
            <div key={ev.seq} style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <span className="num" style={{ fontSize: 12, color: 'var(--muted)', minWidth: 150 }}>
                {new Date(ev.at_unix_ms).toLocaleString('ru-RU')}
              </span>
              <span style={{ color: 'var(--ink-soft)' }}>{EVENT_LABEL[ev.kind] ?? ev.kind}</span>
            </div>
          ))}
        </div>
      )}
    </Card>
  );
}

function CopyButton({ label, value }: { label: string; value: string }) {
  const [done, setDone] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard?.writeText(value);
      setDone(true);
      window.setTimeout(() => setDone(false), 1500);
    } catch {
      // Буфер обмена недоступен — молча игнорируем (не критично).
    }
  }
  return (
    <Button variant="secondary" style={NEUTRAL_BTN} onClick={() => void copy()}>
      {done ? 'Скопировано ✓' : label}
    </Button>
  );
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}
