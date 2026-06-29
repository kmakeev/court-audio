//! Watchdog конвейера захвата — этап 02 (`promts/02_recorder_reliability.md`,
//! deliverable 4).
//!
//! Контролирует «живость» захвата по [`Heartbeat`] — отметке времени последнего
//! поступления семплов (обновляется в `cpal`-callback'е и/или consumer'е). Если
//! семплы не приходят дольше `reliability.watchdog_timeout_ms` — запись «молча
//! встала»: watchdog вызывает инъектируемое действие `on_stall` (перезапуск
//! потока захвата + журнал `WatchdogRestart` + событие UI).
//!
//! Решение «зависло ли» вынесено в чистую функцию [`is_stalled`] — она
//! тестируется без потоков и без `cpal`. Само действие рестарта инъектируется,
//! поэтому фоновый цикл тоже проверяется в CI без устройства.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Текущее монотонно-настенное время в мс от эпохи Unix.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Разделяемая отметка «последнего поступления семплов» (мс от эпохи Unix).
/// Клонируется (Arc) между producer/consumer и watchdog-потоком.
#[derive(Debug, Clone)]
pub struct Heartbeat(Arc<AtomicU64>);

impl Heartbeat {
    /// Создать heartbeat, инициализированный текущим временем.
    pub fn new() -> Self {
        Self(Arc::new(AtomicU64::new(now_unix_ms())))
    }

    /// Отметить «живость» текущим временем (вызывается при поступлении семплов).
    pub fn beat(&self) {
        self.0.store(now_unix_ms(), Ordering::Relaxed);
    }

    /// Отметить произвольным временем (для тестов).
    pub fn beat_at(&self, unix_ms: u64) {
        self.0.store(unix_ms, Ordering::Relaxed);
    }

    /// Последняя отметка (мс).
    pub fn last(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}

/// Зависла ли запись: с последней отметки прошло больше таймаута. Чистая
/// функция — инъекция `now`/`last` делает её тестируемой без потоков.
pub fn is_stalled(now_ms: u64, last_ms: u64, timeout_ms: u64) -> bool {
    now_ms.saturating_sub(last_ms) > timeout_ms
}

/// Дескриптор работающего watchdog-потока.
pub struct Watchdog {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Watchdog {
    /// Запустить фоновый watchdog. Каждые `poll` он проверяет heartbeat; при
    /// простое дольше `timeout` вызывает `on_stall` **один раз на залипание**
    /// (дебаунс), пока запись не «оживёт» вновь.
    pub fn spawn<F>(
        heartbeat: Heartbeat,
        timeout: Duration,
        poll: Duration,
        mut on_stall: F,
    ) -> Self
    where
        F: FnMut() + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let timeout_ms = timeout.as_millis() as u64;

        let handle = thread::Builder::new()
            .name("reliability-watchdog".into())
            .spawn(move || {
                let mut fired = false;
                while !stop_for_thread.load(Ordering::Acquire) {
                    let stalled = is_stalled(now_unix_ms(), heartbeat.last(), timeout_ms);
                    if stalled && !fired {
                        on_stall();
                        fired = true; // дебаунс: не дёргаем рестарт каждый poll
                    } else if !stalled {
                        fired = false; // запись ожила — снова вооружаемся
                    }
                    thread::sleep(poll);
                }
            })
            .expect("не удалось запустить поток watchdog");

        Self {
            stop,
            handle: Some(handle),
        }
    }

    /// Остановить watchdog и дождаться завершения потока.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    #[test]
    fn is_stalled_pure_logic() {
        // now - last <= timeout -> жив; > timeout -> завис.
        assert!(!is_stalled(1_000, 1_000, 500));
        assert!(!is_stalled(1_500, 1_000, 500));
        assert!(is_stalled(1_501, 1_000, 500));
        // Защита от «отрицательного» интервала (часы прыгнули назад).
        assert!(!is_stalled(900, 1_000, 500));
    }

    #[test]
    fn heartbeat_beat_updates_last() {
        let hb = Heartbeat::new();
        hb.beat_at(42);
        assert_eq!(hb.last(), 42);
        let clone = hb.clone();
        clone.beat_at(99);
        assert_eq!(hb.last(), 99); // общий Arc
    }

    #[test]
    fn spawn_fires_on_stall_and_debounces() {
        // Heartbeat «застыл» в прошлом -> watchdog должен сработать.
        let hb = Heartbeat::new();
        hb.beat_at(now_unix_ms().saturating_sub(10_000)); // далеко в прошлом

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_cb = Arc::clone(&calls);
        let wd = Watchdog::spawn(
            hb.clone(),
            Duration::from_millis(50),
            Duration::from_millis(10),
            move || {
                calls_cb.fetch_add(1, Ordering::Relaxed);
            },
        );

        // Ждём несколько циклов опроса.
        let start = Instant::now();
        while calls.load(Ordering::Relaxed) == 0 && start.elapsed() < Duration::from_secs(2) {
            thread::sleep(Duration::from_millis(10));
        }
        // Сработал хотя бы раз...
        assert!(calls.load(Ordering::Relaxed) >= 1);
        // ...но из-за дебанса не на каждом poll (не «бесконечно»).
        thread::sleep(Duration::from_millis(100));
        assert!(calls.load(Ordering::Relaxed) <= 2);
        wd.stop();
    }

    #[test]
    fn spawn_does_not_fire_while_alive() {
        let hb = Heartbeat::new(); // свежий heartbeat = «жив»
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_cb = Arc::clone(&calls);
        let wd = Watchdog::spawn(
            hb.clone(),
            Duration::from_millis(100),
            Duration::from_millis(10),
            move || {
                calls_cb.fetch_add(1, Ordering::Relaxed);
            },
        );
        // Поддерживаем «жизнь» некоторое время.
        for _ in 0..10 {
            hb.beat();
            thread::sleep(Duration::from_millis(10));
        }
        wd.stop();
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }
}
