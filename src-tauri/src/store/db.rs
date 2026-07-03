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
pub const SCHEMA_VERSION: i64 = 5;

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
    if version < 3 {
        // Этап 09 (многоканал по ролям): дорожки с ролями + per-track сегменты/
        // выгрузка. Добавляем `tracks`; в `segments`/`upload_parts` вводим
        // `track_id` (пересбор с новым PK). Легаси-сессии (моно) получают трек
        // `track_id=0` роль `single`, а их сегменты/части — `track_id=0`.
        // Таблицы `segments`/`upload_parts` ничем не референсятся — пересбор
        // через *_new/DROP/RENAME безопасен при foreign_keys=ON.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tracks (
                session_id       TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                track_id         INTEGER NOT NULL,
                role             TEXT NOT NULL,
                label            TEXT NOT NULL DEFAULT '',
                source_device    TEXT,
                source_channel   INTEGER NOT NULL DEFAULT 0,
                final_chain_link TEXT,
                PRIMARY KEY (session_id, track_id)
            );
            -- Легаси-сессии: одна дорожка track_id=0.
            INSERT OR IGNORE INTO tracks (session_id, track_id, role, label, source_channel)
                SELECT id, 0, 'single', '', 0 FROM sessions;

            -- Пересбор segments с track_id и PK (session_id, track_id, idx).
            CREATE TABLE segments_new (
                session_id          TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                track_id            INTEGER NOT NULL DEFAULT 0,
                idx                 INTEGER NOT NULL,
                path                TEXT NOT NULL,
                started_at_unix_ms  INTEGER NOT NULL,
                frames              INTEGER NOT NULL,
                size_bytes          INTEGER NOT NULL,
                sha256              TEXT NOT NULL,
                chain_link          TEXT NOT NULL,
                PRIMARY KEY (session_id, track_id, idx)
            );
            INSERT INTO segments_new (session_id, track_id, idx, path, started_at_unix_ms,
                                      frames, size_bytes, sha256, chain_link)
                SELECT session_id, 0, idx, path, started_at_unix_ms,
                       frames, size_bytes, sha256, chain_link FROM segments;
            DROP TABLE segments;
            ALTER TABLE segments_new RENAME TO segments;

            -- Пересбор upload_parts с track_id и PK (session_id, track_id, part_index).
            CREATE TABLE upload_parts_new (
                session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                track_id    INTEGER NOT NULL DEFAULT 0,
                part_index  INTEGER NOT NULL,
                size_bytes  INTEGER NOT NULL,
                sha256      TEXT NOT NULL,
                state       TEXT NOT NULL,
                attempts    INTEGER NOT NULL DEFAULT 0,
                last_error  TEXT,
                PRIMARY KEY (session_id, track_id, part_index)
            );
            INSERT INTO upload_parts_new (session_id, track_id, part_index, size_bytes,
                                          sha256, state, attempts, last_error)
                SELECT session_id, 0, part_index, size_bytes,
                       sha256, state, attempts, last_error FROM upload_parts;
            DROP TABLE upload_parts;
            ALTER TABLE upload_parts_new RENAME TO upload_parts;",
        )?;
        conn.pragma_update(None, "user_version", 3)?;
    }
    if version < 4 {
        // Этап 10 (метки/роли): живая разметка — append-only журнал действий под
        // хеш-цепочкой. `seq` монотонен в рамках сессии; `chain_link` вычислен
        // при постановке (реконсиляция не пере-хеширует). Легаси-сессии без
        // разметки просто не имеют строк — таблица аддитивна.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS annotations (
                session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                seq             INTEGER NOT NULL,
                action          TEXT NOT NULL,
                target_id       TEXT NOT NULL,
                category        TEXT,
                role            TEXT,
                comment         TEXT,
                offset_samples  INTEGER NOT NULL,
                offset_ms       INTEGER NOT NULL,
                operator_id     TEXT NOT NULL,
                at_unix_ms      INTEGER NOT NULL,
                chain_link      TEXT NOT NULL,
                PRIMARY KEY (session_id, seq)
            );",
        )?;
        conn.pragma_update(None, "user_version", 4)?;
    }
    if version < 5 {
        // Этап 10.4 (разграничение доступа): станционный журнал изменений
        // настроек. Не в `events` — та привязана FK к `sessions`, а изменения
        // настроек станционные (вне сессии). Append-only; `seq` — автоинкремент;
        // `changes_json` — поле-уровневый diff (старое→новое, без секретов).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings_audit (
                seq               INTEGER PRIMARY KEY AUTOINCREMENT,
                at_unix_ms        INTEGER NOT NULL,
                actor_operator_id TEXT NOT NULL,
                source            TEXT NOT NULL,
                dangerous         INTEGER NOT NULL DEFAULT 0,
                changes_json      TEXT NOT NULL
            );",
        )?;
        conn.pragma_update(None, "user_version", 5)?;
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
            "tracks",
            "annotations",
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
        // Симулируем БД схемы v1: базовые таблицы (как их создаёт ветка v1) +
        // user_version=1. Включаем segments/events — реальная v1-БД их имеет, а
        // цепочка миграций до v3 их пересобирает.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY, dir TEXT NOT NULL, started_at_unix_ms INTEGER NOT NULL,
                status TEXT NOT NULL, station_id TEXT NOT NULL, operator_id TEXT NOT NULL,
                adjudication_ref TEXT, sample_rate_hz INTEGER NOT NULL, channels INTEGER NOT NULL,
                bit_depth INTEGER NOT NULL, final_chain_link TEXT, upload_status TEXT NOT NULL,
                server_integrity_verified INTEGER NOT NULL DEFAULT 0, confirmed_at_unix_ms INTEGER,
                local_purged_at_unix_ms INTEGER
            );
            CREATE TABLE segments (
                session_id TEXT NOT NULL, idx INTEGER NOT NULL, path TEXT NOT NULL,
                started_at_unix_ms INTEGER NOT NULL, frames INTEGER NOT NULL,
                size_bytes INTEGER NOT NULL, sha256 TEXT NOT NULL, chain_link TEXT NOT NULL,
                PRIMARY KEY (session_id, idx)
            );
            CREATE TABLE events (
                session_id TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL,
                at_unix_ms INTEGER NOT NULL, detail_json TEXT, PRIMARY KEY (session_id, seq)
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
    fn migrate_v2_to_v3_backfills_track_and_preserves_segments() {
        // Схема v2: сессия + сегмент + часть выгрузки, track_id ещё нет.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY, dir TEXT NOT NULL, started_at_unix_ms INTEGER NOT NULL,
                status TEXT NOT NULL, station_id TEXT NOT NULL, operator_id TEXT NOT NULL,
                adjudication_ref TEXT, sample_rate_hz INTEGER NOT NULL, channels INTEGER NOT NULL,
                bit_depth INTEGER NOT NULL, final_chain_link TEXT, upload_status TEXT NOT NULL,
                server_integrity_verified INTEGER NOT NULL DEFAULT 0, confirmed_at_unix_ms INTEGER,
                local_purged_at_unix_ms INTEGER, upload_paused INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE segments (
                session_id TEXT NOT NULL, idx INTEGER NOT NULL, path TEXT NOT NULL,
                started_at_unix_ms INTEGER NOT NULL, frames INTEGER NOT NULL,
                size_bytes INTEGER NOT NULL, sha256 TEXT NOT NULL, chain_link TEXT NOT NULL,
                PRIMARY KEY (session_id, idx)
            );
            CREATE TABLE upload_parts (
                session_id TEXT NOT NULL, part_index INTEGER NOT NULL, size_bytes INTEGER NOT NULL,
                sha256 TEXT NOT NULL, state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT, PRIMARY KEY (session_id, part_index)
            );
            INSERT INTO sessions (id, dir, started_at_unix_ms, status, station_id, operator_id,
                sample_rate_hz, channels, bit_depth, upload_status)
                VALUES ('s1','/rec/s1',1,'stopped','st','op',44100,1,16,'pending');
            INSERT INTO segments (session_id, idx, path, started_at_unix_ms, frames, size_bytes, sha256, chain_link)
                VALUES ('s1',1,'seg1.wav',1,1000,2000,'h1','l1');
            INSERT INTO upload_parts (session_id, part_index, size_bytes, sha256, state)
                VALUES ('s1',1,2000,'h1','pending');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();

        migrate(&conn).unwrap();

        // Легаси-сессия получила дорожку track_id=0 роль single.
        let (tid, role): (i64, String) = conn
            .query_row(
                "SELECT track_id, role FROM tracks WHERE session_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(tid, 0);
        assert_eq!(role, "single");
        // Сегмент цел и получил track_id=0.
        let (stid, sha): (i64, String) = conn
            .query_row(
                "SELECT track_id, sha256 FROM segments WHERE session_id='s1' AND idx=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(stid, 0);
        assert_eq!(sha, "h1");
        // Часть выгрузки цела и получила track_id=0.
        let ptid: i64 = conn
            .query_row(
                "SELECT track_id FROM upload_parts WHERE session_id='s1' AND part_index=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ptid, 0);
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn migrate_v3_to_v4_adds_annotations_and_preserves_data() {
        // Схема v3: сессия + дорожка + сегмент; таблицы annotations ещё нет.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY, dir TEXT NOT NULL, started_at_unix_ms INTEGER NOT NULL,
                status TEXT NOT NULL, station_id TEXT NOT NULL, operator_id TEXT NOT NULL,
                adjudication_ref TEXT, sample_rate_hz INTEGER NOT NULL, channels INTEGER NOT NULL,
                bit_depth INTEGER NOT NULL, final_chain_link TEXT, upload_status TEXT NOT NULL,
                server_integrity_verified INTEGER NOT NULL DEFAULT 0, confirmed_at_unix_ms INTEGER,
                local_purged_at_unix_ms INTEGER, upload_paused INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO sessions (id, dir, started_at_unix_ms, status, station_id, operator_id,
                sample_rate_hz, channels, bit_depth, upload_status)
                VALUES ('s1','/rec/s1',1,'stopped','st','op',44100,1,16,'pending');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();

        migrate(&conn).unwrap();

        // Таблица annotations появилась, сессия цела.
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='annotations'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let sid: String = conn
            .query_row("SELECT id FROM sessions WHERE id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sid, "s1");
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn migrate_v4_to_v5_adds_settings_audit() {
        // Схема v4: settings_audit ещё нет.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY, dir TEXT NOT NULL, started_at_unix_ms INTEGER NOT NULL,
                status TEXT NOT NULL, station_id TEXT NOT NULL, operator_id TEXT NOT NULL,
                adjudication_ref TEXT, sample_rate_hz INTEGER NOT NULL, channels INTEGER NOT NULL,
                bit_depth INTEGER NOT NULL, final_chain_link TEXT, upload_status TEXT NOT NULL,
                server_integrity_verified INTEGER NOT NULL DEFAULT 0, confirmed_at_unix_ms INTEGER,
                local_purged_at_unix_ms INTEGER, upload_paused INTEGER NOT NULL DEFAULT 0
            );",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 4).unwrap();

        migrate(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='settings_audit'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
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
