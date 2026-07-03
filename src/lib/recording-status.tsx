// Общий источник состояния записи (этап 10.5). Один провайдер подписывается на
// события ядра (снимок `capture_status` + живые `capture_state`/`audio_level`) и
// раздаёт их всем «всегда-на-виду» индикаторам: бейдж в шапке, «режим зала»,
// компакт-оверлей. Экран «Запись» ведёт свою (более широкую) логику отдельно —
// здесь только то, что нужно индикаторам.
import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import {
  getCaptureStatus,
  onAudioLevel,
  onCaptureState,
  type CaptureStateValue,
  type LevelEvent,
} from './core';

// Такт обновления хронометра — только отображение (раз в секунду).
const CLOCK_TICK_MS = 1000;

/** Человекочитаемая метка состояния записи (шапка/режим зала/оверлей). */
export const RECORDING_STATUS_LABEL: Record<CaptureStateValue, string> = {
  idle: 'Готов к записи',
  recording: 'Идёт запись',
  paused: 'Пауза',
  stopping: 'Остановка…',
  stopped: 'Запись завершена',
};

/** Тон бейджа (PravoUI `Tag`) для состояния записи. */
export const RECORDING_STATUS_TONE: Record<
  CaptureStateValue,
  'default' | 'accent' | 'gold' | 'green'
> = {
  idle: 'default',
  recording: 'accent',
  paused: 'gold',
  stopping: 'default',
  stopped: 'green',
};

export function recordingStatusLabel(state: CaptureStateValue): string {
  return RECORDING_STATUS_LABEL[state];
}

export function recordingStatusTone(
  state: CaptureStateValue,
): 'default' | 'accent' | 'gold' | 'green' {
  return RECORDING_STATUS_TONE[state];
}

/** Идёт ли активная сессия (запись/пауза/остановка) — индикатор виден. */
export function isSessionActive(state: CaptureStateValue): boolean {
  return state === 'recording' || state === 'paused' || state === 'stopping';
}

export interface RecordingStatus {
  state: CaptureStateValue;
  startedAtMs: number | null;
  elapsedSec: number;
  /** Уровни по дорожкам: track_id → последнее событие (одноканал — ключ 0). */
  levels: Record<number, LevelEvent>;
}

const DEFAULT_STATUS: RecordingStatus = {
  state: 'idle',
  startedAtMs: null,
  elapsedSec: 0,
  levels: {},
};

const RecordingStatusContext = createContext<RecordingStatus>(DEFAULT_STATUS);

/**
 * Провайдер общего состояния записи. Монтируется в оболочке приложения; жив,
 * пока открыто окно (события идут из ядра, переживают переходы между экранами).
 */
export function RecordingStatusProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<CaptureStateValue>('idle');
  const [startedAtMs, setStartedAtMs] = useState<number | null>(null);
  const [elapsedSec, setElapsedSec] = useState(0);
  const [levels, setLevels] = useState<Record<number, LevelEvent>>({});

  // Стартовый снимок: запись могла идти в фоне до монтирования провайдера.
  useEffect(() => {
    let active = true;
    getCaptureStatus()
      .then((s) => {
        if (!active) return;
        if (s.state === 'recording' || s.state === 'paused') {
          setState(s.state);
          setStartedAtMs(s.started_at_unix_ms);
        }
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, []);

  // Подписки на живые события состояния и уровня.
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    let active = true;
    const wire = async () => {
      const a = await onCaptureState((e) => {
        setState(e.state);
        if (e.state === 'recording') {
          // Функциональное обновление: не сбрасываем метку старта при
          // возобновлении (resume тоже эмитит `recording`), ставим только при
          // первом старте (когда метки ещё нет).
          setStartedAtMs((prev) => prev ?? Date.now());
        } else if (e.state === 'idle' || e.state === 'stopped') {
          setStartedAtMs(null);
          setLevels({});
        }
      });
      const b = await onAudioLevel((e) =>
        setLevels((prev) => ({ ...prev, [e.track_id ?? 0]: e })),
      );
      if (active) {
        unlisteners.push(a, b);
      } else {
        a();
        b();
      }
    };
    void wire();
    return () => {
      active = false;
      unlisteners.forEach((u) => u());
    };
  }, []);

  // Хронометр: тикаем, пока сессия активна (запись/пауза).
  useEffect(() => {
    if ((state !== 'recording' && state !== 'paused') || startedAtMs == null) {
      setElapsedSec(0);
      return;
    }
    const tick = () =>
      setElapsedSec(Math.max(0, Math.floor((Date.now() - startedAtMs) / 1000)));
    tick();
    const id = window.setInterval(tick, CLOCK_TICK_MS);
    return () => window.clearInterval(id);
  }, [state, startedAtMs]);

  const value = useMemo<RecordingStatus>(
    () => ({ state, startedAtMs, elapsedSec, levels }),
    [state, startedAtMs, elapsedSec, levels],
  );

  return (
    <RecordingStatusContext.Provider value={value}>
      {children}
    </RecordingStatusContext.Provider>
  );
}

/** Прочитать общее состояние записи (внутри `RecordingStatusProvider`). */
export function useRecordingStatus(): RecordingStatus {
  return useContext(RecordingStatusContext);
}
