//! Движок локального ретеншна (этап 03 — `promts/03_store_integrity.md`,
//! deliverable 6 / шаг 7).
//!
//! Политика удаления локальной копии — зеркало серверного
//! `purge_expired_uploads`. По `Settings.retention.*`:
//! - `until_confirmed_plus_window` (дефолт) — удалять только после серверного
//!   подтверждения целостности **и** истечения окна безопасности;
//! - `delete_on_confirm` — сразу по подтверждению (минимизация ПДн);
//! - `manual` — никогда автоматически.
//!
//! Сам триггер «сервер подтвердил» приходит из `06` ([`mark_server_confirmed`]).
//! Здесь — политика ([`RetentionPolicy`]), безопасное удаление ([`safe_delete`])
//! и планировщик-обход ([`sweep`]). Все пороги — из настроек, без «магических
//! чисел».

use std::path::{Path, PathBuf};

use crate::settings::{RetentionMode, RetentionSettings};

use super::manifest::{ManifestStore, SessionRecord, SessionStatus};
use super::StoreError;

/// Миллисекунд в часе — единица перевода `safety_window_hours` в метку времени.
const MS_PER_HOUR: u64 = 3_600_000;

/// Политика ретеншна, разрешённая из настроек.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    mode: RetentionMode,
    require_integrity_verified: bool,
    safety_window_hours: u32,
}

/// Решение по сессии.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Можно удалять локальную копию.
    Delete,
    /// Удалять нельзя — с причиной (для диагностики/журнала).
    Keep(KeepReason),
}

/// Причина, по которой локальная копия удерживается.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepReason {
    /// Режим `manual` — только ручное удаление.
    ManualMode,
    /// Локальная копия уже удалена (tombstone).
    AlreadyPurged,
    /// Сервер ещё не подтвердил приём.
    NotConfirmed,
    /// Сервер не подтвердил целостность (`require_integrity_verified`).
    IntegrityNotVerified,
    /// Окно безопасности после подтверждения ещё не истекло.
    WindowNotElapsed,
}

impl RetentionPolicy {
    /// Построить политику из настроек.
    pub fn from_settings(s: &RetentionSettings) -> Self {
        Self {
            mode: s.mode,
            require_integrity_verified: s.require_integrity_verified,
            safety_window_hours: s.safety_window_hours,
        }
    }

    /// Можно ли удалять локальную копию сессии на момент `now_unix_ms`.
    pub fn decide(&self, session: &SessionRecord, now_unix_ms: u64) -> Decision {
        if session.status == SessionStatus::Purged || session.local_purged_at_unix_ms.is_some() {
            return Decision::Keep(KeepReason::AlreadyPurged);
        }
        if self.mode == RetentionMode::Manual {
            return Decision::Keep(KeepReason::ManualMode);
        }

        // Триггер из `06`: подтверждение приёма сервером.
        let Some(confirmed_at) = session.confirmed_at_unix_ms else {
            return Decision::Keep(KeepReason::NotConfirmed);
        };
        if self.require_integrity_verified && !session.server_integrity_verified {
            return Decision::Keep(KeepReason::IntegrityNotVerified);
        }

        match self.mode {
            RetentionMode::DeleteOnConfirm => Decision::Delete,
            RetentionMode::UntilConfirmedPlusWindow => {
                let window_ms = self.safety_window_hours as u64 * MS_PER_HOUR;
                if now_unix_ms >= confirmed_at.saturating_add(window_ms) {
                    Decision::Delete
                } else {
                    Decision::Keep(KeepReason::WindowNotElapsed)
                }
            }
            // Manual обработан выше.
            RetentionMode::Manual => Decision::Keep(KeepReason::ManualMode),
        }
    }
}

/// Триггер подтверждения из `06`: сервер принял запись (и, опционально,
/// верифицировал целостность). Открывает отсчёт окна ретеншна.
pub fn mark_server_confirmed(
    store: &ManifestStore,
    session_id: &str,
    integrity_verified: bool,
    at_unix_ms: u64,
) -> Result<(), StoreError> {
    store.mark_server_confirmed(session_id, integrity_verified, at_unix_ms)
}

/// Безопасно удалить локальную копию сессии: файлы сегментов + строки
/// `segments`/`events`, затем пометить сессию `Purged` (tombstone для истории
/// UI остаётся). Общую соль (`key.salt`) не трогаем — она нужна другим сессиям.
pub fn safe_delete(
    store: &ManifestStore,
    session: &SessionRecord,
    now_unix_ms: u64,
) -> Result<(), StoreError> {
    let session_dir = PathBuf::from(&session.dir);
    for seg in store.get_segments(&session.id)? {
        let path = resolve_segment_path(&session_dir, &seg.path);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            // Уже удалён — идемпотентность важнее.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(StoreError::Io(e.to_string())),
        }
    }
    store.delete_segments_and_events(&session.id)?;
    store.mark_purged(&session.id, now_unix_ms)?;
    Ok(())
}

/// Планировщик-обход: удалить все сессии, подпадающие под политику на момент
/// `now_unix_ms`. Возвращает идентификаторы удалённых. Вызывается из app-цикла
/// (не на горячем пути записи).
pub fn sweep(
    store: &ManifestStore,
    settings: &RetentionSettings,
    now_unix_ms: u64,
) -> Result<Vec<String>, StoreError> {
    let policy = RetentionPolicy::from_settings(settings);
    let mut purged = Vec::new();
    for session in store.list_sessions()? {
        if policy.decide(&session, now_unix_ms) == Decision::Delete {
            safe_delete(store, &session, now_unix_ms)?;
            purged.push(session.id);
        }
    }
    Ok(purged)
}

/// Абсолютный путь сегмента: если в манифесте записан относительный путь —
/// раскрываем относительно каталога сессии.
fn resolve_segment_path(session_dir: &Path, stored: &str) -> PathBuf {
    let p = Path::new(stored);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        session_dir.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::RetentionSettings;
    use crate::store::manifest::SegmentRecord;

    fn settings(mode: RetentionMode, require: bool, window_hours: u32) -> RetentionSettings {
        RetentionSettings {
            mode,
            require_integrity_verified: require,
            safety_window_hours: window_hours,
        }
    }

    fn session(id: &str, dir: &str) -> SessionRecord {
        SessionRecord::new(id, dir, 1_000, "station-1", "operator-1", 44_100, 1, 16)
    }

    #[test]
    fn not_deletable_before_confirmation() {
        let policy = RetentionPolicy::from_settings(&settings(
            RetentionMode::UntilConfirmedPlusWindow,
            true,
            72,
        ));
        let s = session("s1", "/rec/s1");
        assert_eq!(
            policy.decide(&s, 9_999_999_999),
            Decision::Keep(KeepReason::NotConfirmed)
        );
    }

    #[test]
    fn not_deletable_if_integrity_unverified() {
        let policy = RetentionPolicy::from_settings(&settings(
            RetentionMode::UntilConfirmedPlusWindow,
            true,
            72,
        ));
        let mut s = session("s1", "/rec/s1");
        s.confirmed_at_unix_ms = Some(1_000);
        s.server_integrity_verified = false;
        assert_eq!(
            policy.decide(&s, 9_999_999_999),
            Decision::Keep(KeepReason::IntegrityNotVerified)
        );
    }

    #[test]
    fn not_deletable_until_window_elapses_then_deletable() {
        let window_hours = 72;
        let policy = RetentionPolicy::from_settings(&settings(
            RetentionMode::UntilConfirmedPlusWindow,
            true,
            window_hours,
        ));
        let mut s = session("s1", "/rec/s1");
        let confirmed_at = 10_000_000u64;
        s.confirmed_at_unix_ms = Some(confirmed_at);
        s.server_integrity_verified = true;

        let window_ms = window_hours as u64 * MS_PER_HOUR;
        // За миг до истечения окна — держим.
        assert_eq!(
            policy.decide(&s, confirmed_at + window_ms - 1),
            Decision::Keep(KeepReason::WindowNotElapsed)
        );
        // По истечении окна — удаляем.
        assert_eq!(
            policy.decide(&s, confirmed_at + window_ms),
            Decision::Delete
        );
    }

    #[test]
    fn delete_on_confirm_ignores_window() {
        let policy =
            RetentionPolicy::from_settings(&settings(RetentionMode::DeleteOnConfirm, true, 72));
        let mut s = session("s1", "/rec/s1");
        s.confirmed_at_unix_ms = Some(1_000);
        s.server_integrity_verified = true;
        // Сразу после подтверждения, окно не учитывается.
        assert_eq!(policy.decide(&s, 1_001), Decision::Delete);
    }

    #[test]
    fn manual_mode_never_deletes() {
        let policy = RetentionPolicy::from_settings(&settings(RetentionMode::Manual, true, 0));
        let mut s = session("s1", "/rec/s1");
        s.confirmed_at_unix_ms = Some(1_000);
        s.server_integrity_verified = true;
        assert_eq!(
            policy.decide(&s, u64::MAX),
            Decision::Keep(KeepReason::ManualMode)
        );
    }

    #[test]
    fn require_false_deletes_on_confirm_without_integrity() {
        let policy =
            RetentionPolicy::from_settings(&settings(RetentionMode::DeleteOnConfirm, false, 72));
        let mut s = session("s1", "/rec/s1");
        s.confirmed_at_unix_ms = Some(1_000);
        s.server_integrity_verified = false; // не требуется
        assert_eq!(policy.decide(&s, 2_000), Decision::Delete);
    }

    #[test]
    fn safe_delete_removes_files_and_rows_keeps_tombstone() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s1");
        std::fs::create_dir_all(&dir).unwrap();
        let seg_path = dir.join("seg-0001.wav.enc");
        std::fs::write(&seg_path, b"ciphertext").unwrap();

        let store = ManifestStore::in_memory().unwrap();
        let s = session("s1", dir.to_str().unwrap());
        store.insert_session(&s).unwrap();
        store
            .append_segment(
                "s1",
                &SegmentRecord {
                    track_id: 0,
                    index: 1,
                    path: seg_path.to_str().unwrap().to_string(),
                    started_at_unix_ms: 1,
                    frames: 1,
                    size_bytes: 10,
                    sha256: "h".into(),
                    chain_link: "l".into(),
                },
            )
            .unwrap();

        safe_delete(&store, &s, 5_000).unwrap();
        assert!(!seg_path.exists());
        assert!(store.get_segments("s1").unwrap().is_empty());
        let back = store.get_session("s1").unwrap().unwrap();
        assert_eq!(back.status, SessionStatus::Purged);
        assert_eq!(back.local_purged_at_unix_ms, Some(5_000));
    }

    #[test]
    fn sweep_deletes_only_eligible() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ManifestStore::in_memory().unwrap();

        // Eligible: подтверждена + верифицирована + окно истекло.
        let dir_ok = tmp.path().join("ok");
        std::fs::create_dir_all(&dir_ok).unwrap();
        let mut ok = session("ok", dir_ok.to_str().unwrap());
        ok.confirmed_at_unix_ms = Some(0);
        ok.server_integrity_verified = true;
        store.insert_session(&ok).unwrap();

        // Не eligible: не подтверждена.
        let pending = session("pending", "/rec/pending");
        store.insert_session(&pending).unwrap();

        let s = settings(RetentionMode::UntilConfirmedPlusWindow, true, 1);
        let now = MS_PER_HOUR + 1; // окно 1 час истекло
        let purged = sweep(&store, &s, now).unwrap();
        assert_eq!(purged, vec!["ok".to_string()]);
        assert_eq!(
            store.get_session("ok").unwrap().unwrap().status,
            SessionStatus::Purged
        );
        assert_eq!(
            store.get_session("pending").unwrap().unwrap().status,
            SessionStatus::Recording
        );
    }

    #[test]
    fn mark_server_confirmed_sets_trigger_fields() {
        let store = ManifestStore::in_memory().unwrap();
        store.insert_session(&session("s1", "/rec/s1")).unwrap();
        mark_server_confirmed(&store, "s1", true, 12_345).unwrap();
        let s = store.get_session("s1").unwrap().unwrap();
        assert!(s.server_integrity_verified);
        assert_eq!(s.confirmed_at_unix_ms, Some(12_345));
    }
}
