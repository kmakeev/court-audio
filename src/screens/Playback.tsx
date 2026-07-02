import { useCallback, useEffect, useMemo, useState } from 'react';
import type { CSSProperties } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { BlockHead, Button, Card, Select, Tag } from '../design';
import {
  closePlaybackSession,
  formatAdjudicationRef,
  onPlayerPosition,
  openPlaybackSession,
  playbackPause,
  playbackPlay,
  playbackSeek,
  selectPlaybackTrack,
  setPlaybackRate,
  setPlaybackVolume,
  type MarkerState,
  type PlayerSessionInfo,
  type RoleSpanState,
  type TrackSelector,
} from '../lib/core';
import { getSettings, type Settings } from '../lib/settings';

// Экран «Прослушивание» (этап 10.1). Открывается кнопкой «Прослушать» из
// списка сессий («Сессии»). Вся дешифровка/склейка/вывод звука — в ядре
// (player_cmds); здесь только команды и отображение позиции/меток.

const NEUTRAL_BTN = { color: 'var(--ink)', borderColor: 'var(--ink-soft)' } as const;
// Одинаковый размер кнопки play/pause независимо от варианта DS (у
// primary/secondary разная высота) — иначе кнопка «прыгает» при переключении.
const TRANSPORT_BTN: CSSProperties = { height: 44, minWidth: 140, justifyContent: 'center' };
// Шаг громкости по стрелкам ↑/↓ — только UX-косметика (не параметр реестра).
const VOLUME_STEP = 0.05;
// Высота оглавления, после которой появляется вертикальный скролл (список
// меток/интервалов может быть длинным — пагинации в DS нет, скролл проще и
// консистентен с выпадающим списком `Select`).
const TOC_MAX_HEIGHT = 360;
// Допуск при сравнении позиции со смещением записи оглавления: ядро
// округляет позицию через два усекающих деления (мс→фрейм→мс, см.
// `player::timeline`), из-за чего эхо-событие после сика может прийти на
// ~1мс МЕНЬШЕ точного смещения самой метки/интервала — без допуска строгое
// сравнение отбрасывало бы её саму же и подсвечивало предыдущую запись.
const HIGHLIGHT_EPSILON_MS = 20;

type Load =
  | { kind: 'loading' }
  | { kind: 'ready'; info: PlayerSessionInfo }
  | { kind: 'error'; message: string };

type Item =
  | { kind: 'marker'; offsetMs: number; marker: MarkerState }
  | { kind: 'role'; offsetMs: number; span: RoleSpanState };

export function PlaybackScreen() {
  const params = useParams();
  const navigate = useNavigate();
  const dir = params.dir ? decodeURIComponent(params.dir) : '';

  const [settings, setSettings] = useState<Settings | null>(null);
  const [load, setLoad] = useState<Load>({ kind: 'loading' });
  const [selector, setSelector] = useState<TrackSelector>({ kind: 'track', track_id: 0 });
  const [playing, setPlaying] = useState(false);
  const [positionMs, setPositionMs] = useState(0);
  const [durationMs, setDurationMs] = useState(0);
  const [rate, setRate] = useState(1);
  const [volume, setVolume] = useState(1);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    Promise.all([getSettings(), openPlaybackSession(dir)])
      .then(([s, info]) => {
        if (!active) return;
        setSettings(s);
        setLoad({ kind: 'ready', info });
        setDurationMs(info.duration_ms);
        setSelector(
          info.tracks[0] ? { kind: 'track', track_id: info.tracks[0].track_id } : { kind: 'mix' },
        );
      })
      .catch((e: unknown) => active && setLoad({ kind: 'error', message: describeError(e) }));
    return () => {
      active = false;
      void closePlaybackSession().catch(() => {});
    };
  }, [dir]);

  // Событие позиции воспроизведения.
  useEffect(() => {
    let active = true;
    const unlisteners: Array<() => void> = [];
    const wire = async () => {
      const un = await onPlayerPosition((e) => {
        setPositionMs(e.position_ms);
        setDurationMs(e.duration_ms);
        setPlaying(e.state === 'playing');
      });
      if (active) unlisteners.push(un);
      else un();
    };
    void wire();
    return () => {
      active = false;
      unlisteners.forEach((u) => u());
    };
  }, []);

  const onPlay = useCallback(() => {
    playbackPlay()
      .then(() => setPlaying(true))
      .catch((e: unknown) => setError(describeError(e)));
  }, []);

  const onPause = useCallback(() => {
    playbackPause()
      .then(() => setPlaying(false))
      .catch((e: unknown) => setError(describeError(e)));
  }, []);

  const onSeekMs = useCallback((ms: number) => {
    const clamped = Math.max(0, Math.round(ms));
    // Оптимистично — не ждём подтверждения от ядра, иначе плейхед/подсветка
    // в оглавлении на мгновение остаются на старой позиции.
    setPositionMs(clamped);
    playbackSeek({ kind: 'ms', ms: clamped }).catch((e: unknown) => setError(describeError(e)));
  }, []);

  const onSeekMarker = useCallback((id: string, offsetMs: number) => {
    setPositionMs(offsetMs);
    playbackSeek({ kind: 'marker', id }).catch((e: unknown) => setError(describeError(e)));
  }, []);

  const onSelectTrack = useCallback((next: TrackSelector) => {
    selectPlaybackTrack(next)
      .then(() => {
        setSelector(next);
        setPositionMs(0);
        setPlaying(false);
      })
      .catch((e: unknown) => setError(describeError(e)));
  }, []);

  const onRateChange = useCallback((r: number) => {
    setPlaybackRate(r)
      .then(() => setRate(r))
      .catch((e: unknown) => setError(describeError(e)));
  }, []);

  const onVolumeChange = useCallback((v: number) => {
    const clamped = Math.max(0, Math.min(1, v));
    setPlaybackVolume(clamped)
      .then(() => setVolume(clamped))
      .catch((e: unknown) => setError(describeError(e)));
  }, []);

  const items: Item[] = useMemo(() => {
    if (load.kind !== 'ready') return [];
    return [
      ...load.info.markers.map((marker): Item => ({
        kind: 'marker',
        offsetMs: marker.offset_ms,
        marker,
      })),
      ...load.info.role_spans.map((span): Item => ({
        kind: 'role',
        offsetMs: span.start_offset_ms,
        span,
      })),
    ].sort((a, b) => a.offsetMs - b.offsetMs);
  }, [load]);

  // Текущая запись оглавления — последняя, чьё смещение уже пройдено
  // позицией воспроизведения (как активная глава в аудио/видео-плеере).
  // Чистая производная величина от (items, positionMs) — никакого таймера/
  // состояния: раньше здесь был дебаунс с `setTimeout`, но его же cleanup на
  // каждое изменение `positionMs` отменял ещё не сработавший таймер, и во
  // время непрерывного тика позиции (события каждые ~200мс) он не успевал
  // сработать НИКОГДА — подсветка «замерзала» на старом значении вместо
  // отслеживания хода воспроизведения. Прямое вычисление на каждый рендер
  // всегда синхронно с `positionMs` и не может «зависнуть».
  const currentItemKey = useMemo(() => {
    let key: string | null = null;
    for (const item of items) {
      if (item.offsetMs > positionMs + HIGHLIGHT_EPSILON_MS) break;
      key = itemKey(item);
    }
    return key;
  }, [items, positionMs]);

  const seekStepMs = Math.round((settings?.player.seek_step_seconds ?? 15) * 1000);

  // Хоткеи: Пробел — play/pause, ←/→ — перемотка ±шаг настройки, ↑/↓ —
  // громкость ±5%. Не перехватываем при фокусе в поле ввода/скраббере —
  // там за это отвечает нативная клавиатурная доступность `<input>`.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) return;
      if (e.key === ' ') {
        e.preventDefault();
        if (playing) onPause();
        else onPlay();
      } else if (e.key === 'ArrowLeft') {
        e.preventDefault();
        onSeekMs(positionMs - seekStepMs);
      } else if (e.key === 'ArrowRight') {
        e.preventDefault();
        onSeekMs(Math.min(durationMs, positionMs + seekStepMs));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        onVolumeChange(volume + VOLUME_STEP);
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        onVolumeChange(volume - VOLUME_STEP);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [playing, positionMs, durationMs, volume, seekStepMs, onPlay, onPause, onSeekMs, onVolumeChange]);

  const backButton = (
    <Button variant="secondary" style={NEUTRAL_BTN} onClick={() => navigate('/sessions')}>
      ← К сессиям
    </Button>
  );

  if (load.kind === 'loading') {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
        <div>{backButton}</div>
        <Card>
          <BlockHead numeral="▶" title="Прослушивание" hint="Загрузка сессии…" />
        </Card>
      </div>
    );
  }

  if (load.kind === 'error') {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
        <div>{backButton}</div>
        <Card>
          <BlockHead numeral="▶" title="Прослушивание" />
          <Tag tone="accent">Ошибка: {load.message}</Tag>
        </Card>
      </div>
    );
  }

  const { info } = load;

  const trackOptions = [
    ...info.tracks.map((t) => ({ value: `track:${t.track_id}`, label: t.label.trim() || t.role })),
    ...(info.tracks.length > 1 ? [{ value: 'mix', label: 'Микс всех дорожек' }] : []),
  ];
  const selectorValue = selector.kind === 'mix' ? 'mix' : `track:${selector.track_id}`;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
      <div>{backButton}</div>

      <Card>
        <BlockHead
          numeral="▶"
          title="Прослушивание"
          hint={formatAdjudicationRef(info.adjudication_ref) ?? 'Без привязки к делу'}
        />
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
          <Tag tone={info.integrity_ok ? 'green' : 'accent'}>
            {info.integrity_ok ? 'Целостность подтверждена' : 'Целостность не подтверждена'}
          </Tag>
          <span className="num" style={{ fontSize: 12, color: 'var(--ink-soft)' }}>
            {formatDateTime(info.started_at_unix_ms)}
          </span>
        </div>
      </Card>

      {error && <Tag tone="accent">Ошибка: {error}</Tag>}

      <Card>
        <BlockHead numeral="A" title="Таймлайн" />
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <span className="num" style={{ fontSize: 13, color: 'var(--ink)', width: 64, flexShrink: 0 }}>
            {formatClock(positionMs)}
          </span>
          {/* Скраббер и полоса меток/ролей — в одном flex:1 контейнере, чтобы
              ось 0–100% совпадала пиксель-в-пиксель у обоих рядов. */}
          <div style={{ flex: 1, minWidth: 0 }}>
            <input
              type="range"
              aria-label="Позиция воспроизведения"
              min={0}
              max={durationMs || 0}
              value={Math.min(positionMs, durationMs || 0)}
              onChange={(e) => setPositionMs(Number(e.target.value))}
              onMouseUp={(e) => onSeekMs(Number((e.target as HTMLInputElement).value))}
              onKeyUp={(e) => {
                if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key)) {
                  onSeekMs(Number((e.target as HTMLInputElement).value));
                }
              }}
              style={{ width: '100%', display: 'block', accentColor: 'var(--accent)' }}
            />
            {/* Метки (штрихи) и интервалы ролей (полоски) на оси сессии — только
                для ориентира, не кликабельны: переход — по строке в оглавлении
                ниже (клики по мелким засечкам ненадёжны при большом числе
                событий и ограниченной ширине шкалы). */}
            {items.length > 0 && (
              <div style={{ position: 'relative', height: 10, marginTop: 4 }} aria-hidden="true">
                {info.role_spans.map((s) => {
                  const endMs = s.end_offset_ms ?? durationMs;
                  const leftPct = pct(s.start_offset_ms, durationMs);
                  const widthPct = Math.max(0.5, pct(endMs, durationMs) - leftPct);
                  return (
                    <span
                      key={s.id}
                      title={`Роль: ${s.role}`}
                      style={{
                        position: 'absolute',
                        left: `${leftPct}%`,
                        width: `${widthPct}%`,
                        top: 0,
                        height: 6,
                        background: 'var(--gold-tint)',
                        border: '1px solid var(--gold)',
                      }}
                    />
                  );
                })}
                {info.markers.map((m) => (
                  <span
                    key={m.id}
                    title={m.comment ? `${m.category}: ${m.comment}` : m.category}
                    style={{
                      position: 'absolute',
                      left: `calc(${pct(m.offset_ms, durationMs)}% - 1px)`,
                      top: 0,
                      width: 2,
                      height: 10,
                      background: 'var(--accent)',
                    }}
                  />
                ))}
              </div>
            )}
          </div>
          <span className="num" style={{ fontSize: 13, color: 'var(--ink-soft)', width: 64, flexShrink: 0 }}>
            {formatClock(durationMs)}
          </span>
        </div>
        {items.length > 0 && (
          <p style={{ fontSize: 11, color: 'var(--muted)', margin: '4px 0 0 76px' }}>
            <span style={{ color: 'var(--accent)' }}>▍</span> метки ·{' '}
            <span style={{ color: 'var(--gold)' }}>▬</span> роли говорящих — см. оглавление ниже
          </p>
        )}

        <div style={{ display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap', marginTop: 20 }}>
          {playing ? (
            <Button variant="secondary" style={{ ...NEUTRAL_BTN, ...TRANSPORT_BTN }} onClick={onPause}>
              ❚❚ Пауза
            </Button>
          ) : (
            <Button variant="primary" style={TRANSPORT_BTN} onClick={onPlay}>
              ▶ Играть
            </Button>
          )}
          <Button
            variant="secondary"
            style={NEUTRAL_BTN}
            onClick={() => onSeekMs(positionMs - seekStepMs)}
          >
            ◀◀ {Math.round(seekStepMs / 1000)} с
          </Button>
          <Button
            variant="secondary"
            style={NEUTRAL_BTN}
            onClick={() => onSeekMs(Math.min(durationMs, positionMs + seekStepMs))}
          >
            {Math.round(seekStepMs / 1000)} с ▶▶
          </Button>

          <div style={{ minWidth: 100 }}>
            <Select
              ariaLabel="Скорость воспроизведения"
              value={String(rate)}
              onChange={(v) => onRateChange(Number(v))}
              options={(settings?.player.playback_rates ?? [1]).map((r) => ({
                value: String(r),
                label: `${r}×`,
              }))}
            />
          </div>

          <label style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            <span style={{ fontSize: 11, color: 'var(--muted)' }}>Громкость</span>
            <input
              type="range"
              aria-label="Громкость"
              min={0}
              max={1}
              step={0.05}
              value={volume}
              onChange={(e) => onVolumeChange(Number(e.target.value))}
              style={{ width: 90, accentColor: 'var(--accent)' }}
            />
          </label>

          {trackOptions.length > 1 && (
            <div style={{ minWidth: 180 }}>
              <Select
                ariaLabel="Дорожка"
                value={selectorValue}
                onChange={(v) =>
                  onSelectTrack(
                    v === 'mix'
                      ? { kind: 'mix' }
                      : { kind: 'track', track_id: Number(v.slice('track:'.length)) },
                  )
                }
                options={trackOptions}
              />
            </div>
          )}
        </div>

        <p style={{ fontSize: 12, color: 'var(--muted)', marginTop: 16, marginBottom: 0 }}>
          Горячие клавиши: <kbd>Пробел</kbd> — play/pause, <kbd>←</kbd>/<kbd>→</kbd> — перемотка,{' '}
          <kbd>↑</kbd>/<kbd>↓</kbd> — громкость.
        </p>
      </Card>

      <Card>
        <BlockHead numeral="B" title={`Оглавление (${items.length})`} />
        {items.length === 0 ? (
          <p style={{ fontSize: 12, color: 'var(--muted)', margin: 0 }}>
            Меток и интервалов ролей в этой сессии нет.
          </p>
        ) : (
          <ul
            style={{
              listStyle: 'none',
              margin: 0,
              padding: 0,
              display: 'flex',
              flexDirection: 'column',
              gap: 8,
              maxHeight: TOC_MAX_HEIGHT,
              overflowY: 'auto',
            }}
          >
            {items.map((item) => {
              const key = itemKey(item);
              const isCurrent = key === currentItemKey;
              return (
                <li key={key}>
                  <button
                    type="button"
                    aria-current={isCurrent ? 'true' : undefined}
                    onClick={() =>
                      onSeekMarker(
                        item.kind === 'marker' ? item.marker.id : item.span.id,
                        item.offsetMs,
                      )
                    }
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 10,
                      width: '100%',
                      background: isCurrent ? 'var(--paper-strong)' : 'transparent',
                      border: 'none',
                      borderBottom: '1px solid var(--hairline)',
                      padding: '6px 0',
                      cursor: 'pointer',
                      textAlign: 'left',
                    }}
                  >
                    <span className="num" style={{ fontSize: 12, color: 'var(--ink)', width: 64 }}>
                      {formatClock(item.offsetMs)}
                    </span>
                    {item.kind === 'marker' ? (
                      <>
                        <Tag tone="default">{item.marker.category}</Tag>
                        {item.marker.comment && (
                          <span style={{ fontSize: 12, color: 'var(--ink-soft)' }}>
                            {item.marker.comment}
                          </span>
                        )}
                      </>
                    ) : (
                      <Tag tone="accent">{item.span.role}</Tag>
                    )}
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </Card>

      <div>{backButton}</div>
    </div>
  );
}

function itemKey(item: Item): string {
  return `${item.kind}-${item.kind === 'marker' ? item.marker.id : item.span.id}`;
}

function pct(offsetMs: number, durationMs: number): number {
  if (durationMs <= 0) return 0;
  return Math.max(0, Math.min(100, (offsetMs / durationMs) * 100));
}

function formatClock(totalMs: number): string {
  const totalSec = Math.max(0, Math.floor(totalMs / 1000));
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(h)}:${pad(m)}:${pad(s)}`;
}

function formatDateTime(unixMs: number): string {
  return new Date(unixMs).toLocaleString('ru-RU');
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}
