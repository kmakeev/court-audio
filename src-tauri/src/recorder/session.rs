//! Конечный автомат сессии записи (этап 02 — `promts/02_recorder_reliability.md`,
//! deliverable 1).
//!
//! Состояния `Idle → Recording ⇄ Paused → Stopping → Stopped`, плюс `Error` и
//! `Recovering`. Автомат **чистый** (без I/O): он только проверяет легальность
//! переходов и хранит причину паузы, чтобы авто-возобновление при возврате
//! устройства срабатывало лишь для паузы из-за обрыва устройства (а не для
//! паузы, поставленной оператором вручную). Журналирование и события UI —
//! на стороне вызывающего (`ipc/`, consumer).
//!
//! Магических чисел здесь нет: автомат оперирует только состояниями/событиями.

use std::fmt;

use serde::Serialize;

/// Состояние сессии записи. Сериализуется в `snake_case` для события
/// `capture_state` (UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Запись не идёт.
    Idle,
    /// Идёт активная запись.
    Recording,
    /// Пауза (оператором или из-за обрыва устройства — см. [`PauseReason`]).
    Paused,
    /// Запрошена остановка: дописываем остаток буфера и финализируем сегменты.
    Stopping,
    /// Сессия завершена.
    Stopped,
    /// Восстановление незавершённой сессии при старте приложения.
    Recovering,
    /// Аварийное состояние (нерешаемая ошибка конвейера).
    Error,
}

/// Причина паузы. Влияет на авто-возобновление: восстанавливаем только паузу,
/// вызванную обрывом устройства.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// Оператор нажал «пауза» — авто-возобновление не применяется.
    Operator,
    /// Устройство пропало — при возврате возможно авто-возобновление.
    DeviceLost,
}

/// Событие, подаваемое автомату.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Старт записи (из `Idle` или после `Recovering`).
    Start,
    /// Пауза оператором.
    Pause,
    /// Возобновление оператором.
    Resume,
    /// Обрыв/пропажа устройства в ходе записи.
    DeviceLost,
    /// Возврат устройства.
    DeviceBack,
    /// Запрос остановки.
    RequestStop,
    /// Конвейер дописал остаток и финализировал сегменты.
    Finalized,
    /// Нерешаемая ошибка конвейера.
    Failed,
    /// Старт восстановления незавершённой сессии.
    BeginRecovery,
    /// Восстановление завершено (готовы продолжить/закрыть).
    RecoveryDone,
}

/// Ошибка некорректного перехода автомата.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionError {
    pub from: SessionState,
    pub event: SessionEvent,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "недопустимый переход: событие {:?} в состоянии {:?}",
            self.event, self.from
        )
    }
}

impl std::error::Error for TransitionError {}

/// Конечный автомат сессии записи.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMachine {
    state: SessionState,
    /// Причина текущей паузы (валидна только в состоянии `Paused`).
    pause_reason: Option<PauseReason>,
}

impl Default for SessionMachine {
    fn default() -> Self {
        Self {
            state: SessionState::Idle,
            pause_reason: None,
        }
    }
}

impl SessionMachine {
    /// Новый автомат в состоянии `Idle`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Текущее состояние.
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Причина текущей паузы (если в паузе).
    pub fn pause_reason(&self) -> Option<PauseReason> {
        self.pause_reason
    }

    /// Должны ли мы авто-возобновиться при возврате устройства: только если на
    /// паузе из-за обрыва устройства и в настройках включён авто-резюм.
    pub fn should_auto_resume(&self, auto_resume_enabled: bool) -> bool {
        auto_resume_enabled
            && self.state == SessionState::Paused
            && self.pause_reason == Some(PauseReason::DeviceLost)
    }

    /// Применить событие. Возвращает новое состояние или ошибку, если переход
    /// недопустим (само состояние при ошибке не меняется).
    pub fn apply(&mut self, event: SessionEvent) -> Result<SessionState, TransitionError> {
        use SessionEvent as E;
        use SessionState as S;

        let next = match (self.state, &event) {
            // Старт записи.
            (S::Idle, E::Start) | (S::Recovering, E::Start) => S::Recording,

            // Восстановление при старте.
            (S::Idle, E::BeginRecovery) => S::Recovering,
            (S::Recovering, E::RecoveryDone) => S::Idle,

            // Пауза/возобновление оператором.
            (S::Recording, E::Pause) => {
                self.pause_reason = Some(PauseReason::Operator);
                S::Paused
            }
            (S::Paused, E::Resume) => {
                self.pause_reason = None;
                S::Recording
            }

            // Обрыв и возврат устройства.
            (S::Recording, E::DeviceLost) => {
                self.pause_reason = Some(PauseReason::DeviceLost);
                S::Paused
            }
            // Возврат устройства снимает «device_lost»; решение о фактическом
            // возобновлении принимает вызывающий (см. should_auto_resume).
            (S::Paused, E::DeviceBack) if self.pause_reason == Some(PauseReason::DeviceLost) => {
                self.pause_reason = None;
                S::Recording
            }

            // Остановка из активных состояний.
            (S::Recording, E::RequestStop) | (S::Paused, E::RequestStop) => {
                self.pause_reason = None;
                S::Stopping
            }
            (S::Stopping, E::Finalized) => S::Stopped,

            // Авария — из любого «живого» состояния.
            (S::Recording, E::Failed)
            | (S::Paused, E::Failed)
            | (S::Stopping, E::Failed)
            | (S::Recovering, E::Failed) => S::Error,

            // Всё остальное — недопустимо.
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
        let m = SessionMachine::new();
        assert_eq!(m.state(), SessionState::Idle);
        assert_eq!(m.pause_reason(), None);
    }

    #[test]
    fn full_record_pause_resume_stop_cycle() {
        let mut m = SessionMachine::new();
        assert_eq!(m.apply(SessionEvent::Start).unwrap(), SessionState::Recording);
        assert_eq!(m.apply(SessionEvent::Pause).unwrap(), SessionState::Paused);
        assert_eq!(m.pause_reason(), Some(PauseReason::Operator));
        assert_eq!(m.apply(SessionEvent::Resume).unwrap(), SessionState::Recording);
        assert_eq!(
            m.apply(SessionEvent::RequestStop).unwrap(),
            SessionState::Stopping
        );
        assert_eq!(
            m.apply(SessionEvent::Finalized).unwrap(),
            SessionState::Stopped
        );
    }

    #[test]
    fn device_lost_pauses_with_reason() {
        let mut m = SessionMachine::new();
        m.apply(SessionEvent::Start).unwrap();
        assert_eq!(
            m.apply(SessionEvent::DeviceLost).unwrap(),
            SessionState::Paused
        );
        assert_eq!(m.pause_reason(), Some(PauseReason::DeviceLost));
        // Авто-резюм применим только при включённой настройке.
        assert!(m.should_auto_resume(true));
        assert!(!m.should_auto_resume(false));
    }

    #[test]
    fn operator_pause_does_not_auto_resume() {
        let mut m = SessionMachine::new();
        m.apply(SessionEvent::Start).unwrap();
        m.apply(SessionEvent::Pause).unwrap();
        // Пауза оператором — авто-резюм не применяется даже при включённой настройке.
        assert!(!m.should_auto_resume(true));
        // И возврат устройства не возобновляет «операторскую» паузу.
        assert!(m.apply(SessionEvent::DeviceBack).is_err());
    }

    #[test]
    fn device_back_resumes_only_device_pause() {
        let mut m = SessionMachine::new();
        m.apply(SessionEvent::Start).unwrap();
        m.apply(SessionEvent::DeviceLost).unwrap();
        assert_eq!(
            m.apply(SessionEvent::DeviceBack).unwrap(),
            SessionState::Recording
        );
        assert_eq!(m.pause_reason(), None);
    }

    #[test]
    fn recovery_flow() {
        let mut m = SessionMachine::new();
        assert_eq!(
            m.apply(SessionEvent::BeginRecovery).unwrap(),
            SessionState::Recovering
        );
        // Можно либо продолжить записью, либо закрыть восстановление.
        let mut cont = m.clone();
        assert_eq!(
            cont.apply(SessionEvent::Start).unwrap(),
            SessionState::Recording
        );
        assert_eq!(
            m.apply(SessionEvent::RecoveryDone).unwrap(),
            SessionState::Idle
        );
    }

    #[test]
    fn failure_from_live_states() {
        for setup in [
            vec![SessionEvent::Start],
            vec![SessionEvent::Start, SessionEvent::Pause],
            vec![SessionEvent::Start, SessionEvent::RequestStop],
            vec![SessionEvent::BeginRecovery],
        ] {
            let mut m = SessionMachine::new();
            for e in setup {
                m.apply(e).unwrap();
            }
            assert_eq!(m.apply(SessionEvent::Failed).unwrap(), SessionState::Error);
        }
    }

    #[test]
    fn illegal_transitions_rejected_without_state_change() {
        let mut m = SessionMachine::new();
        // Из Idle нельзя паузить/возобновлять/останавливать.
        for e in [
            SessionEvent::Pause,
            SessionEvent::Resume,
            SessionEvent::RequestStop,
            SessionEvent::DeviceLost,
            SessionEvent::Finalized,
        ] {
            assert!(m.apply(e).is_err());
            assert_eq!(m.state(), SessionState::Idle);
        }
    }

    #[test]
    fn no_restart_from_stopped() {
        let mut m = SessionMachine::new();
        m.apply(SessionEvent::Start).unwrap();
        m.apply(SessionEvent::RequestStop).unwrap();
        m.apply(SessionEvent::Finalized).unwrap();
        assert_eq!(m.state(), SessionState::Stopped);
        assert!(m.apply(SessionEvent::Start).is_err());
    }
}
