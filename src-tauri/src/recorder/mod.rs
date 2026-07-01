//! Конечный автомат сессии записи (этап 02 — `promts/02_recorder_reliability.md`).
//!
//! Состояния сессии (idle → recording → paused → stopped), непрерывный сброс
//! на диск короткими сегментами (`Settings.recorder.segment_seconds`), fsync с
//! интервалом `flush_interval_ms`, ротация сегментов и защита от «бесконечной»
//! сессии (`max_session_hours`). Ядро принципа «бесперебойности».
//!
//! Этап 01: реализован [`segment_writer`] — сегментный WAV-райтер с непрерывным
//! сбросом и ротацией (потребитель кольцевого буфера). Этап 02 добавляет:
//! - [`session`] — конечный автомат сессии (idle/recording/paused/…);
//! - [`journal`] — аварийно-устойчивый журнал восстановления (write-ahead);
//! - [`recovery`] — обнаружение и починка незавершённой сессии при старте.

pub mod journal;
pub mod multitrack;
pub mod recovery;
pub mod segment_writer;
pub mod session;
