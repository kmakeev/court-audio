//! Монитор устройства ввода — этап 02 (`promts/02_recorder_reliability.md`,
//! deliverable 5).
//!
//! Отслеживает появление/пропажу выбранного устройства и драйвит конечный
//! автомат сессии: пропажа → пауза с пометкой `device_lost`; возврат при
//! `reliability.device_reconnect.auto_resume=true` → авто-возобновление.
//!
//! Наличие устройства — **инъектируемый пробник** (`Fn() -> bool`): в живом коде
//! это поиск `audio.device` среди `cpal::input_devices`, в тестах — замыкание.
//! Поэтому логика тестируется в CI без реального устройства.

use crate::recorder::session::{SessionEvent, SessionMachine, SessionState};

/// Событие изменения присутствия устройства.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceEvent {
    /// Устройство пропало.
    Lost,
    /// Устройство вернулось.
    Back,
}

/// Монитор присутствия устройства. Хранит последнее известное состояние и при
/// опросе выдаёт переход present↔absent (если он произошёл).
pub struct DeviceMonitor<P: Fn() -> bool> {
    probe: P,
    present: bool,
}

impl<P: Fn() -> bool> DeviceMonitor<P> {
    /// Создать монитор; начальное состояние снимается пробником сразу.
    pub fn new(probe: P) -> Self {
        let present = probe();
        Self { probe, present }
    }

    /// Текущее (последнее наблюдённое) присутствие.
    pub fn is_present(&self) -> bool {
        self.present
    }

    /// Опросить пробник и вернуть событие, если присутствие изменилось.
    pub fn poll(&mut self) -> Option<DeviceEvent> {
        let now = (self.probe)();
        if now == self.present {
            return None;
        }
        self.present = now;
        Some(if now {
            DeviceEvent::Back
        } else {
            DeviceEvent::Lost
        })
    }
}

/// Применить событие устройства к автомату сессии с учётом настройки
/// авто-возобновления. Возвращает новое состояние, если переход случился.
///
/// - `Lost` в `Recording` → `Paused(device_lost)`.
/// - `Back` в `Paused(device_lost)` и `auto_resume=true` → `Recording`; иначе
///   ничего не меняем (оператор возобновит вручную в UI).
pub fn apply_device_event(
    machine: &mut SessionMachine,
    event: DeviceEvent,
    auto_resume: bool,
) -> Option<SessionState> {
    match event {
        DeviceEvent::Lost => {
            if machine.state() == SessionState::Recording {
                return machine.apply(SessionEvent::DeviceLost).ok();
            }
            None
        }
        DeviceEvent::Back => {
            if machine.should_auto_resume(auto_resume) {
                return machine.apply(SessionEvent::DeviceBack).ok();
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn poll_detects_transitions_only() {
        let present = Cell::new(true);
        let mut mon = DeviceMonitor::new(|| present.get());
        // Без изменений — None.
        assert_eq!(mon.poll(), None);
        // Пропажа.
        present.set(false);
        assert_eq!(mon.poll(), Some(DeviceEvent::Lost));
        assert!(!mon.is_present());
        // Повторный опрос без изменений — None.
        assert_eq!(mon.poll(), None);
        // Возврат.
        present.set(true);
        assert_eq!(mon.poll(), Some(DeviceEvent::Back));
    }

    #[test]
    fn lost_then_back_auto_resumes_when_enabled() {
        let mut m = SessionMachine::new();
        m.apply(SessionEvent::Start).unwrap();

        let s = apply_device_event(&mut m, DeviceEvent::Lost, true);
        assert_eq!(s, Some(SessionState::Paused));

        let s = apply_device_event(&mut m, DeviceEvent::Back, true);
        assert_eq!(s, Some(SessionState::Recording));
    }

    #[test]
    fn back_does_not_resume_when_auto_resume_disabled() {
        let mut m = SessionMachine::new();
        m.apply(SessionEvent::Start).unwrap();
        apply_device_event(&mut m, DeviceEvent::Lost, false);
        assert_eq!(m.state(), SessionState::Paused);

        // Авто-резюм выключен — остаёмся на паузе (оператор возобновит вручную).
        let s = apply_device_event(&mut m, DeviceEvent::Back, false);
        assert_eq!(s, None);
        assert_eq!(m.state(), SessionState::Paused);
    }

    #[test]
    fn lost_ignored_when_not_recording() {
        let mut m = SessionMachine::new(); // Idle
        assert_eq!(apply_device_event(&mut m, DeviceEvent::Lost, true), None);
        assert_eq!(m.state(), SessionState::Idle);
    }
}
