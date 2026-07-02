//! Контроль целостности (этап 03 — `promts/03_store_integrity.md`).
//!
//! SHA-256 по сегментам (`Settings.integrity.segment_hash`), хеш-цепочка между
//! сегментами (`integrity.hash_chain`) и журнал событий записи
//! (старт/пауза/обрыв/…, `integrity.event_log`). ГОСТ ЭЦП (КриптоПро) —
//! фаза 2 (`integrity.gost_sign`), здесь не реализуется.

pub mod annotations;
pub mod events;
pub mod hash;
