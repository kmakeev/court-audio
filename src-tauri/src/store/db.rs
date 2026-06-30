//! Открытие SQLite-манифеста и схема (этап 03 — `promts/03_store_integrity.md`,
//! шаг 1).
//!
//! `rusqlite` (bundled — без системного libsqlite, ради импортозамещаемой
//! сборки). Журнал — WAL: устойчивость к сбою на самом манифесте и параллельное
//! чтение для UI. Версия схемы — в `PRAGMA user_version`; миграции идемпотентны
//! (повторный вызов безопасен).

use std::path::Path;

use rusqlite::Connection;

use super::StoreError;

/// Текущая версия схемы манифеста. При изменении таблиц — инкремент + ветка в
/// [`migrate`].
pub const SCHEMA_VERSION: i64 = 2;

/// Открыть (создав при необходимости) манифест-БД по пути и применить миграции.
pub fn open(path: &Path) -> Result<Connection, StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    init(&conn)?;
    Ok(conn)
}

/// Манифест в памяти (для тестов; WAL на `:memory:` игнорируется SQLite).
pub fn open_in_memory() -> Result<Connection, StoreError> {
    let conn = Connection::open_in_memory()?;
    init(&conn)?;
    Ok(conn)
}

fn init(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    migrate(conn)?;
    Ok(())
}

/// Применить миграции схемы до [`SCHEMA_VERSION`]. Идемпотентно.
pub fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id                        TEXT PRIMARY KEY,
                dir                       TEXT NOT NULL,
                started_at_unix_ms        INTEGER NOT NULL,
                status                    TEXT NOT NULL,
                station_id                TEXT NOT NULL,
                operator_id               TEXT NOT NULL,
                adjudication_ref          TEXT,
                sample_rate_hz            INTEGER NOT NULL,
                channels                  INTEGER NOT NULL,
                bit_depth                 INTEGER NOT NULL,
                final_chain_link          TEXT,
                upload_status             TEXT NOT NULL,
                server_integrity_verified INTEGER NOT NULL DEFAULT 0,
                confirmed_at_unix_ms      INTEGER,
                local_purged_at_unix_ms   INTEGER
            );
            CREATE TABLE IF NOT EXISTS segments (
                session_id          TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                idx                 INTEGER NOT NULL,
                path                TEXT NOT NULL,
                started_at_unix_ms  INTEGER NOT NULL,
                frames              INTEGER NOT NULL,
                size_bytes          INTEGER NOT NULL,
                sha256              TEXT NOT NULL,
                chain_link          TEXT NOT NULL,
                PRIMARY KEY (session_id, idx)
            );
            CREATE TABLE IF NOT EXISTS events (
                session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                seq         INTEGER NOT NULL,
                kind        TEXT NOT NULL,
                at_unix_ms  INTEGER NOT NULL,
                detail_json TEXT,
                PRIMARY KEY (session_id, seq)
            );",
        )?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    if version < 2 {
        // Этап 06 (sync_agent): трекинг выгрузки. `upload_paused` — операторская
        // пауза догрузки; `upload_state` — серверный recording_id и факт init;
        // `upload_parts` — по-сегментная докачка (часть = сегмент) и идемпотентность.
        conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN upload_paused INTEGER NOT NULL DEFAULT 0;
            CREATE TABLE IF NOT EXISTS upload_state (
                session_id          TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                server_recording_id TEXT,
                init_done           INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS upload_parts (
                session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                part_index  INTEGER NOT NULL,
                size_bytes  INTEGER NOT NULL,
                sha256      TEXT NOT NULL,
                state       TEXT NOT NULL,
                attempts    INTEGER NOT NULL DEFAULT 0,
                last_error  TEXT,
                PRIMARY KEY (session_id, part_index)
            );",
        )?;
        conn.pragma_update(None, "user_version", 2)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_sets_schema_version() {
        let conn = open_in_memory().unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = open_in_memory().unwrap();
        // Повторный вызов не должен падать и не менять версию.
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn tables_exist() {
        let conn = open_in_memory().unwrap();
        for table in [
            "sessions",
            "segments",
            "events",
            "upload_state",
            "upload_parts",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "таблица {table} должна существовать");
        }
    }

    #[test]
    fn migrate_v1_to_v2_preserves_sessions_and_adds_tables() {
        // Симулируем БД схемы v1: только базовые таблицы + user_version=1.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY, dir TEXT NOT NULL, started_at_unix_ms INTEGER NOT NULL,
                status TEXT NOT NULL, station_id TEXT NOT NULL, operator_id TEXT NOT NULL,
                adjudication_ref TEXT, sample_rate_hz INTEGER NOT NULL, channels INTEGER NOT NULL,
                bit_depth INTEGER NOT NULL, final_chain_link TEXT, upload_status TEXT NOT NULL,
                server_integrity_verified INTEGER NOT NULL DEFAULT 0, confirmed_at_unix_ms INTEGER,
                local_purged_at_unix_ms INTEGER
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, dir, started_at_unix_ms, status, station_id, operator_id,
                sample_rate_hz, channels, bit_depth, upload_status)
             VALUES ('s1','/rec/s1',1,'stopped','st','op',44100,1,16,'pending')",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();

        migrate(&conn).unwrap();

        // Данные сессии целы, новая колонка имеет дефолт, новые таблицы есть.
        let paused: i64 = conn
            .query_row(
                "SELECT upload_paused FROM sessions WHERE id='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(paused, 0);
        for table in ["upload_state", "upload_parts"] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "таблица {table} должна появиться после миграции");
        }
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn open_on_disk_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("manifest.sqlite");
        {
            let _conn = open(&path).unwrap();
        }
        assert!(path.exists());
        // Повторное открытие видит ту же версию схемы (миграция не пересоздаёт).
        let conn = open(&path).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }
}
