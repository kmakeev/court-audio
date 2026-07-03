import { useEffect, useRef } from 'react';
import { Tag } from '../design';
import { aggregateLevel, formatClock, toMeterPct } from '../lib/format';
import {
  isSessionActive,
  recordingStatusLabel,
  recordingStatusTone,
  useRecordingStatus,
} from '../lib/recording-status';

// «Режим зала» (этап 10.5): полноэкранная панель крупного статуса — хронометр,
// состояние записи и уровень сигнала читаются с нескольких метров. Источник —
// общий `useRecordingStatus` (тот же, что у индикатора в шапке). Модальна:
// `aria-modal`, фокус переводится на кнопку закрытия, Esc закрывает.

export function HallMode({ onClose }: { onClose: () => void }) {
  const { state, elapsedSec, levels } = useRecordingStatus();
  const closeRef = useRef<HTMLButtonElement>(null);
  const level = aggregateLevel(levels);
  const rmsPct = toMeterPct(level.rms);
  const peakPct = toMeterPct(level.peak);
  const active = state === 'recording';

  // Модальность: перевести фокус на кнопку закрытия, Esc закрывает.
  useEffect(() => {
    closeRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Режим зала — статус записи"
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 100,
        background: 'var(--dark)',
        color: 'var(--on-dark)',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 28,
        padding: '5vh 6vw',
      }}
    >
      <button
        ref={closeRef}
        type="button"
        onClick={onClose}
        style={{
          position: 'absolute',
          top: 20,
          right: 24,
          background: 'transparent',
          border: '1px solid var(--on-dark-soft, #ffffff55)',
          color: 'var(--on-dark)',
          fontFamily: 'var(--sans)',
          fontSize: 14,
          padding: '8px 16px',
          cursor: 'pointer',
        }}
      >
        Свернуть · Esc
      </button>

      <Tag
        tone={recordingStatusTone(state)}
        role="status"
        aria-live="polite"
        style={{ fontSize: 20, padding: '8px 20px' }}
      >
        {active && (
          <span
            aria-hidden="true"
            style={{
              display: 'inline-block',
              width: 12,
              height: 12,
              borderRadius: '50%',
              background: 'var(--accent)',
              marginRight: 8,
            }}
          />
        )}
        {recordingStatusLabel(state)}
      </Tag>

      <div
        className="num"
        aria-label="Хронометраж записи"
        style={{
          fontSize: 'clamp(64px, 18vw, 200px)',
          fontVariantNumeric: 'tabular-nums',
          lineHeight: 1,
          letterSpacing: '0.02em',
          color: 'var(--on-dark)',
        }}
      >
        {formatClock(elapsedSec)}
      </div>

      {/* Крупный метр уровня — виден при активной сессии. */}
      {isSessionActive(state) && (
        <div style={{ width: 'min(720px, 80vw)' }}>
          <div
            role="meter"
            aria-label="Уровень сигнала"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(rmsPct)}
            style={{
              position: 'relative',
              height: 40,
              background: 'var(--dark-soft)',
              border: '1px solid var(--on-dark-soft, #ffffff55)',
              overflow: 'hidden',
            }}
          >
            <div
              className="level-meter-fill"
              style={{
                position: 'absolute',
                inset: 0,
                width: `${rmsPct}%`,
                background: active ? 'var(--green)' : 'var(--muted-soft)',
              }}
            />
            <div
              aria-hidden="true"
              style={{
                position: 'absolute',
                top: 0,
                bottom: 0,
                left: `calc(${peakPct}% - 1px)`,
                width: 2,
                background: 'var(--on-dark)',
              }}
            />
          </div>
          <span
            className="num"
            style={{ fontSize: 14, color: 'var(--on-dark-soft, #ffffffaa)' }}
          >
            RMS {Math.round(rmsPct)}% · пик {Math.round(peakPct)}%
          </span>
        </div>
      )}
    </div>
  );
}
