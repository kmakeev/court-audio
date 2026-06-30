//! Серверная верификация целостности + сигнал ретеншну (этап 06 —
//! `promts/06_sync_agent.md`, шаг 4 / deliverable 5).
//!
//! После `complete` запрашиваем `verify`; сервер пересчитывает SHA-256 по
//! сегментам и хеш-цепочку (контракт `07`). Развилка:
//! - `integrity_verified = true` → дёргаем **триггер ретеншна**
//!   ([`crate::store::retention::mark_server_confirmed`]): фиксируется момент
//!   подтверждения и статус `Confirmed` — это и есть условие удаления локальной
//!   копии по `until_confirmed_plus_window`.
//! - `integrity_verified = false` (подмена сегмента) → статус
//!   [`UploadStatus::IntegrityFailed`]; `confirmed_at` **не ставим**, чтобы при
//!   `require_integrity_verified = false` ретеншн всё равно не удалил подделанную
//!   копию. Локальная копия сохраняется.

use crate::store::manifest::{ManifestStore, UploadStatus};
use crate::store::retention;

use super::client::UploadTransport;
use super::SyncError;

/// Запросить `verify` и применить развилку (триггер ретеншна / статус ошибки
/// целостности). Возвращает фактический `integrity_verified`.
pub fn run_verify(
    transport: &dyn UploadTransport,
    token: &str,
    store: &ManifestStore,
    recording_id: &str,
    session_id: &str,
    now_unix_ms: u64,
) -> Result<bool, SyncError> {
    let outcome = transport.verify(token, recording_id)?;
    if outcome.integrity_verified {
        // Триггер ретеншна (`03`): подтверждение приёма + целостности.
        retention::mark_server_confirmed(store, session_id, true, now_unix_ms)?;
    } else {
        store.set_upload_status(session_id, UploadStatus::IntegrityFailed)?;
    }
    Ok(outcome.integrity_verified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{RetentionMode, RetentionSettings};
    use crate::store::manifest::{SessionRecord, SessionStatus};
    use crate::store::retention::{Decision, RetentionPolicy};
    use crate::sync::testkit::{FakeConfig, FakeTransport};

    fn seed(store: &ManifestStore) {
        let mut s = SessionRecord::new("s1", "/rec/s1", 1, "st", "op", 44_100, 1, 16);
        s.status = SessionStatus::Stopped;
        s.upload_status = UploadStatus::Uploaded;
        store.insert_session(&s).unwrap();
    }

    #[test]
    fn verified_true_triggers_retention_and_confirmed() {
        let store = ManifestStore::in_memory().unwrap();
        seed(&store);
        let t = FakeTransport::happy();
        let verified = run_verify(&t, "tok", &store, "rec-1", "s1", 10_000).unwrap();
        assert!(verified);

        let s = store.get_session("s1").unwrap().unwrap();
        assert_eq!(s.upload_status, UploadStatus::Confirmed);
        assert!(s.server_integrity_verified);
        assert_eq!(s.confirmed_at_unix_ms, Some(10_000));

        // По истечении окна ретеншн может удалять (триггер открыл отсчёт).
        let policy = RetentionPolicy::from_settings(&RetentionSettings {
            mode: RetentionMode::UntilConfirmedPlusWindow,
            require_integrity_verified: true,
            safety_window_hours: 1,
        });
        let after_window = 10_000 + 3_600_000;
        assert_eq!(policy.decide(&s, after_window), Decision::Delete);
    }

    #[test]
    fn verified_false_keeps_copy_even_without_integrity_requirement() {
        let store = ManifestStore::in_memory().unwrap();
        seed(&store);
        let t = FakeTransport::new(FakeConfig {
            verify_result: false,
            ..Default::default()
        });
        let verified = run_verify(&t, "tok", &store, "rec-1", "s1", 10_000).unwrap();
        assert!(!verified);

        let s = store.get_session("s1").unwrap().unwrap();
        assert_eq!(s.upload_status, UploadStatus::IntegrityFailed);
        assert!(!s.server_integrity_verified);
        // Главное: confirmed_at не выставлен → ретеншн не удалит даже при
        // require_integrity_verified = false.
        assert_eq!(s.confirmed_at_unix_ms, None);
        let policy = RetentionPolicy::from_settings(&RetentionSettings {
            mode: RetentionMode::DeleteOnConfirm,
            require_integrity_verified: false,
            safety_window_hours: 0,
        });
        assert!(matches!(policy.decide(&s, u64::MAX), Decision::Keep(_)));
    }
}
