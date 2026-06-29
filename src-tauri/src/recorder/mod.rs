//! Конечный автомат сессии записи (этап 02 — `promts/02_recorder_reliability.md`).
//!
//! Состояния сессии (idle → recording → paused → stopped), непрерывный сброс
//! на диск короткими сегментами (`Settings.recorder.segment_seconds`), fsync с
//! интервалом `flush_interval_ms`, ротация сегментов и защита от «бесконечной»
//! сессии (`max_session_hours`). Ядро принципа «бесперебойности».
//!
//! На этапе 00 — заготовка; логики записи нет.
