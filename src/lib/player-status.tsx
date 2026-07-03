// Общий источник состояния проигрывателя (этап 10.5). Аналог `recording-status`:
// один провайдер подписывается на события ядра проигрывателя (снимок
// `player_status` + живые `player_position`/`player_closed`) и раздаёт их
// индикаторам, которым нужно «фоновое» состояние воспроизведения — прежде всего
// компакт-оверлею, чтобы управлять плеером из окна поверх других программ.
import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import {
  getPlayerStatus,
  onPlayerClosed,
  onPlayerPosition,
} from './core';

export type PlayerPlayState = 'playing' | 'paused' | 'stopped';

export interface PlayerStatusValue {
  /** Открыта ли сессия в проигрывателе (есть чем управлять). */
  active: boolean;
  positionMs: number;
  durationMs: number;
  playState: PlayerPlayState;
}

const DEFAULT: PlayerStatusValue = {
  active: false,
  positionMs: 0,
  durationMs: 0,
  playState: 'stopped',
};

const PlayerStatusContext = createContext<PlayerStatusValue>(DEFAULT);

export function PlayerStatusProvider({ children }: { children: ReactNode }) {
  const [value, setValue] = useState<PlayerStatusValue>(DEFAULT);

  // Стартовый снимок: воспроизведение могло идти до монтирования (напр. окно
  // оверлея открылось во время игры).
  useEffect(() => {
    let active = true;
    getPlayerStatus()
      .then((s) => {
        if (active && s.active) {
          setValue({
            active: true,
            positionMs: s.position_ms,
            durationMs: s.duration_ms,
            playState: s.state,
          });
        }
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, []);

  // Живые события: позиция/состояние во время игры (и снимки на open/seek/pause)
  // + закрытие сессии.
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    let active = true;
    const wire = async () => {
      const a = await onPlayerPosition((e) =>
        setValue({
          active: true,
          positionMs: e.position_ms,
          durationMs: e.duration_ms,
          playState: e.state,
        }),
      );
      const b = await onPlayerClosed(() => setValue(DEFAULT));
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

  const memo = useMemo(() => value, [value]);
  return (
    <PlayerStatusContext.Provider value={memo}>
      {children}
    </PlayerStatusContext.Provider>
  );
}

/** Прочитать общее состояние проигрывателя (внутри `PlayerStatusProvider`). */
export function usePlayerStatus(): PlayerStatusValue {
  return useContext(PlayerStatusContext);
}
