//! Конечный автомат плеера (этап 10.1, шаг 1) — по образцу
//! [`crate::recorder::session::SessionMachine`]: чистый, без I/O, легальность
//! переходов проверяется независимо от Tauri-слоя (`ipc::player_cmds`).

use std::fmt;

use serde::Serialize;

/// Состояние проигрывателя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerState {
    /// Сессия не открыта.
    Idle,
    /// Идёт воспроизведение.
    Playing,
    /// Открыта, воспроизведение приостановлено.
    Paused,
    /// Сессия открыта, но воспроизведение ещё не начато после `Open`, либо
    /// достигнут конец сессии.
    Stopped,
}

/// Событие, подаваемое автомату.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerEvent {
    /// Открыта сессия (готова к воспроизведению, само оно не запущено).
    Open,
    /// Нажато «play».
    Play,
    /// Нажато «пауза».
    Pause,
    /// Перемотка (не меняет play/pause; легальна в `Playing`/`Paused`).
    Seek,
    /// Достигнут конец сессии.
    Ended,
    /// Сессия закрыта (уход с экрана / открытие другой сессии).
    Close,
}

/// Ошибка недопустимого перехода.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionError {
    pub from: PlayerState,
    pub event: PlayerEvent,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "недопустимый переход плеера: событие {:?} в состоянии {:?}",
            self.event, self.from
        )
    }
}

impl std::error::Error for TransitionError {}

/// Конечный автомат плеера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerMachine {
    state: PlayerState,
}

impl Default for PlayerMachine {
    fn default() -> Self {
        Self {
            state: PlayerState::Idle,
        }
    }
}

impl PlayerMachine {
    /// Новый автомат в состоянии `Idle`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Текущее состояние.
    pub fn state(&self) -> PlayerState {
        self.state
    }

    /// Применить событие. При ошибке состояние не меняется.
    pub fn apply(&mut self, event: PlayerEvent) -> Result<PlayerState, TransitionError> {
        use PlayerEvent as E;
        use PlayerState as S;

        let next = match (self.state, event) {
            (S::Idle, E::Open) => S::Stopped,

            (S::Stopped, E::Play) | (S::Paused, E::Play) => S::Playing,
            (S::Playing, E::Pause) => S::Paused,

            // Сик не меняет play/pause. Разрешён и из `Stopped` (сессия открыта,
            // но воспроизведение ещё не начато/уже закончилось) — переход по
            // оглавлению меток до нажатия play не должен быть ошибкой.
            (S::Playing, E::Seek) => S::Playing,
            (S::Paused, E::Seek) => S::Paused,
            (S::Stopped, E::Seek) => S::Stopped,

            (S::Playing, E::Ended) => S::Stopped,

            // Закрыть можно из любого «открытого» состояния.
            (S::Playing, E::Close) | (S::Paused, E::Close) | (S::Stopped, E::Close) => S::Idle,

            _ => {
                return Err(TransitionError {
                    from: self.state,
                    event,
                })
            }
        };

        self.state = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_idle() {
        assert_eq!(PlayerMachine::new().state(), PlayerState::Idle);
    }

    #[test]
    fn open_play_pause_seek_ended_close_cycle() {
        let mut m = PlayerMachine::new();
        assert_eq!(m.apply(PlayerEvent::Open).unwrap(), PlayerState::Stopped);
        assert_eq!(m.apply(PlayerEvent::Play).unwrap(), PlayerState::Playing);
        assert_eq!(m.apply(PlayerEvent::Seek).unwrap(), PlayerState::Playing);
        assert_eq!(m.apply(PlayerEvent::Pause).unwrap(), PlayerState::Paused);
        assert_eq!(m.apply(PlayerEvent::Seek).unwrap(), PlayerState::Paused);
        assert_eq!(m.apply(PlayerEvent::Play).unwrap(), PlayerState::Playing);
        assert_eq!(m.apply(PlayerEvent::Ended).unwrap(), PlayerState::Stopped);
        assert_eq!(m.apply(PlayerEvent::Close).unwrap(), PlayerState::Idle);
    }

    #[test]
    fn cannot_play_from_idle() {
        let mut m = PlayerMachine::new();
        assert!(m.apply(PlayerEvent::Play).is_err());
        assert_eq!(m.state(), PlayerState::Idle);
    }

    #[test]
    fn cannot_seek_before_open() {
        let mut m = PlayerMachine::new();
        assert!(m.apply(PlayerEvent::Seek).is_err());
    }

    #[test]
    fn seek_allowed_while_stopped_right_after_open() {
        // Регрессия: клик по оглавлению меток до первого play (сессия только
        // открыта, воспроизведение ещё не начато) не должен быть ошибкой.
        let mut m = PlayerMachine::new();
        m.apply(PlayerEvent::Open).unwrap();
        assert_eq!(m.apply(PlayerEvent::Seek).unwrap(), PlayerState::Stopped);
        // И после естественного конца (Ended → Stopped) — тоже можно.
        m.apply(PlayerEvent::Play).unwrap();
        m.apply(PlayerEvent::Ended).unwrap();
        assert_eq!(m.apply(PlayerEvent::Seek).unwrap(), PlayerState::Stopped);
    }

    #[test]
    fn close_from_any_open_state_returns_to_idle() {
        for setup in [
            vec![PlayerEvent::Open],
            vec![PlayerEvent::Open, PlayerEvent::Play],
            vec![PlayerEvent::Open, PlayerEvent::Play, PlayerEvent::Pause],
        ] {
            let mut m = PlayerMachine::new();
            for e in setup {
                m.apply(e).unwrap();
            }
            assert_eq!(m.apply(PlayerEvent::Close).unwrap(), PlayerState::Idle);
        }
    }
}
