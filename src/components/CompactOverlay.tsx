import { useCallback, useEffect, useState } from 'react';
import { Tag } from '../design';
import {
  closeCompactOverlay,
  playbackPause,
  playbackPlay,
  playbackSeek,
} from '../lib/core';
import { getSettings } from '../lib/settings';
import { aggregateLevel, formatClock, toMeterPct } from '../lib/format';
import {
  isSessionActive,
  recordingStatusLabel,
  recordingStatusTone,
  RecordingStatusProvider,
  useRecordingStatus,
} from '../lib/recording-status';
import { PlayerStatusProvider, usePlayerStatus } from '../lib/player-status';

// Содержимое компакт-окна статуса «поверх всех окон» (этап 10.5). Отдельное
// Tauri-окно (`?window=overlay`), рендерится из `main.tsx` вне основного App.
// Два режима: если идёт воспроизведение — управление проигрывателем (позиция,
// play/pause, перемотка, горячие клавиши); иначе — статус записи. Так оператор
// работает в другом приложении, не теряя контроль над записью или плейбеком.

const DEFAULT_SEEK_STEP_SEC = 15;

export function CompactOverlayRoot() {
  return (
    <RecordingStatusProvider>
      <PlayerStatusProvider>
        <CompactOverlayBody />
      </PlayerStatusProvider>
    </RecordingStatusProvider>
  );
}

function CompactOverlayBody() {
  const player = usePlayerStatus();
  // Воспроизведение имеет приоритет отображения (запрос оператора): пока сессия
  // открыта в проигрывателе — окно управляет плеером; иначе показывает запись.
  return (
    <div style={shellStyle}>
      {/* Полоса перетаскивания: окно без системной рамки двигается за неё
          (Tauri обрабатывает `data-tauri-drag-region`). Кнопки/транспорт ниже
          не входят в drag-регион — их клики срабатывают как обычно. */}
      <div data-tauri-drag-region style={dragStripStyle} aria-hidden title="Перетащите окно">
        <span data-tauri-drag-region style={dragGripStyle} />
      </div>
      {player.active ? <PlaybackMode /> : <RecordingMode />}
    </div>
  );
}

// ── Режим воспроизведения ────────────────────────────────────────────────────
function PlaybackMode() {
  const player = usePlayerStatus();
  const recording = useRecordingStatus();
  const [seekStepMs, setSeekStepMs] = useState(DEFAULT_SEEK_STEP_SEC * 1000);
  const playing = player.playState === 'playing';

  // Шаг перемотки — из реестра (`player.seek_step_seconds`).
  useEffect(() => {
    let active = true;
    getSettings()
      .then((s) => {
        if (active) setSeekStepMs(Math.round(s.player.seek_step_seconds * 1000));
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, []);

  const onToggle = useCallback(() => {
    (playing ? playbackPause() : playbackPlay()).catch(() => {});
  }, [playing]);

  const onSeek = useCallback(
    (deltaMs: number) => {
      const ms = Math.max(0, Math.min(player.durationMs, player.positionMs + deltaMs));
      playbackSeek({ kind: 'ms', ms }).catch(() => {});
    },
    [player.durationMs, player.positionMs],
  );

  // Горячие клавиши (окно в фокусе): Пробел — play/pause, ←/→ — перемотка.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === ' ') {
        e.preventDefault();
        onToggle();
      } else if (e.key === 'ArrowLeft') {
        e.preventDefault();
        onSeek(-seekStepMs);
      } else if (e.key === 'ArrowRight') {
        e.preventDefault();
        onSeek(seekStepMs);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onToggle, onSeek, seekStepMs]);

  const progressPct =
    player.durationMs > 0 ? Math.min(100, (player.positionMs / player.durationMs) * 100) : 0;

  return (
    <>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <Tag tone={playing ? 'accent' : 'gold'} role="status" aria-live="polite">
          {playing ? '▶ Воспроизведение' : '❚❚ Пауза'}
        </Tag>
        <CloseButton />
      </div>

      <div
        className="num"
        aria-label="Позиция воспроизведения"
        style={{ fontSize: 34, fontVariantNumeric: 'tabular-nums', lineHeight: 1, color: 'var(--on-dark)' }}
      >
        {formatClock(Math.floor(player.positionMs / 1000))}
        <span style={{ fontSize: 15, color: 'var(--on-dark-soft, #ffffffaa)' }}>
          {' / '}
          {formatClock(Math.floor(player.durationMs / 1000))}
        </span>
      </div>

      {/* Дорожка прогресса (без плавности — позиция обновляется событиями). */}
      <div
        role="progressbar"
        aria-label="Прогресс воспроизведения"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(progressPct)}
        style={{
          position: 'relative',
          height: 6,
          background: 'var(--dark-soft)',
          border: '1px solid var(--on-dark-soft, #ffffff55)',
          overflow: 'hidden',
        }}
      >
        <div style={{ position: 'absolute', inset: 0, width: `${progressPct}%`, background: 'var(--green)' }} />
      </div>

      <div style={{ display: 'flex', gap: 6 }}>
        <OverlayButton onClick={() => onSeek(-seekStepMs)} label="Назад" title="Перемотать назад (←)">
          «
        </OverlayButton>
        <OverlayButton onClick={onToggle} label={playing ? 'Пауза' : 'Играть'} title="Пробел" primary>
          {playing ? '❚❚' : '▶'}
        </OverlayButton>
        <OverlayButton onClick={() => onSeek(seekStepMs)} label="Вперёд" title="Перемотать вперёд (→)">
          »
        </OverlayButton>
      </div>

      {/* Если параллельно идёт запись — не прячем её из виду (безопасность). */}
      {isSessionActive(recording.state) && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 12, color: 'var(--on-dark-soft, #ffffffaa)' }}>
          <span aria-hidden style={{ width: 8, height: 8, borderRadius: '50%', background: 'var(--accent)' }} />
          Идёт запись · <span className="num">{formatClock(recording.elapsedSec)}</span>
        </div>
      )}
    </>
  );
}

// ── Режим записи (как раньше) ─────────────────────────────────────────────────
function RecordingMode() {
  const { state, elapsedSec, levels } = useRecordingStatus();
  const rmsPct = toMeterPct(aggregateLevel(levels).rms);
  const active = state === 'recording';

  return (
    <>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <Tag tone={recordingStatusTone(state)} role="status" aria-live="polite">
          {active && (
            <span
              aria-hidden="true"
              style={{
                display: 'inline-block',
                width: 8,
                height: 8,
                borderRadius: '50%',
                background: 'var(--accent)',
                marginRight: 6,
              }}
            />
          )}
          {recordingStatusLabel(state)}
        </Tag>
        <CloseButton />
      </div>

      <div
        className="num"
        aria-label="Хронометраж записи"
        style={{ fontSize: 40, fontVariantNumeric: 'tabular-nums', lineHeight: 1, color: 'var(--on-dark)' }}
      >
        {formatClock(elapsedSec)}
      </div>

      {isSessionActive(state) && (
        <div
          role="meter"
          aria-label="Уровень сигнала"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(rmsPct)}
          style={{
            position: 'relative',
            height: 12,
            background: 'var(--dark-soft)',
            border: '1px solid var(--on-dark-soft, #ffffff55)',
            overflow: 'hidden',
          }}
        >
          <div
            className="level-meter-fill"
            style={{ position: 'absolute', inset: 0, width: `${rmsPct}%`, background: active ? 'var(--green)' : 'var(--muted-soft)' }}
          />
        </div>
      )}
    </>
  );
}

function CloseButton() {
  return (
    <button
      type="button"
      onClick={() => {
        closeCompactOverlay().catch(() => {});
      }}
      aria-label="Закрыть окно статуса"
      title="Закрыть"
      style={{
        marginLeft: 'auto',
        background: 'transparent',
        border: 0,
        color: 'var(--on-dark-soft, #ffffffaa)',
        fontSize: 16,
        cursor: 'pointer',
        lineHeight: 1,
      }}
    >
      ✕
    </button>
  );
}

function OverlayButton({
  onClick,
  label,
  title,
  primary,
  children,
}: {
  onClick: () => void;
  label: string;
  title: string;
  primary?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={title}
      style={{
        flex: primary ? 1 : 'none',
        minWidth: 40,
        height: 34,
        background: primary ? 'var(--accent)' : 'transparent',
        border: '1px solid var(--on-dark-soft, #ffffff55)',
        color: 'var(--on-dark)',
        fontSize: 15,
        cursor: 'pointer',
      }}
    >
      {children}
    </button>
  );
}

const shellStyle = {
  display: 'flex',
  flexDirection: 'column',
  gap: 8,
  // Меньше отступ сверху — над контентом идёт drag-полоса.
  padding: '2px 14px 12px',
  minHeight: '100vh',
  background: 'var(--dark)',
  color: 'var(--on-dark)',
  fontFamily: 'var(--sans)',
} as const;

// Полоса перетаскивания окна (frameless): по центру — «ручка» для наглядности.
const dragStripStyle = {
  display: 'flex',
  alignItems: 'center',
  justifyContent: 'center',
  height: 14,
  cursor: 'grab',
  flex: 'none',
} as const;

const dragGripStyle = {
  width: 36,
  height: 4,
  borderRadius: 2,
  background: 'var(--on-dark-soft, #ffffff55)',
} as const;
