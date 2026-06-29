//! Локальное хранилище и манифест (этап 03 — `promts/03_store_integrity.md`).
//!
//! SQLite-манифест сессий и сегментов (write-ahead журнал состояния),
//! шифрование записей at-rest (`Settings.storage.encrypt_at_rest`), корень
//! хранилища (`storage.root_path`) и ретеншн локальных копий
//! (`Settings.retention.*`) — зеркало серверного `purge_expired_uploads`.
//!
//! На этапе 00 — заготовка; хранилища нет.
