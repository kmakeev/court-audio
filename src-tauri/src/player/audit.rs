//! Аудит доступа к прослушиванию (этап 10.1, deliverable 4): каждое открытие
//! сессии в проигрывателе — событие журнала (`EventKind::PlaybackAccessed`),
//! аудио — ПДн. Пишем напрямую в SQLite-манифест, а не через write-ahead
//! журнал сессии, как для recording-событий: к моменту прослушивания сессия
//! уже `Stopped`/`Recovered`, активного `Journal` для неё нет, а
//! crash-recovery (ради которой существует журнал) для read-доступа не
//! нужна — манифест уже единственный источник истины для завершённой сессии.

use crate::integrity::events::{EventKind, RecordingEvent};
use crate::store::manifest::ManifestStore;
#[cfg(test)]
use crate::store::manifest::SessionRecord;
use crate::store::StoreError;

/// Зафиксировать открытие сессии в проигрывателе: кто (`operator_id`), когда
/// (`at_unix_ms`), какая сессия (`session_id`).
pub fn record_access(
    store: &ManifestStore,
    session_id: &str,
    operator_id: &str,
    at_unix_ms: u64,
) -> Result<(), StoreError> {
    let event = RecordingEvent::with_detail(
        EventKind::PlaybackAccessed,
        at_unix_ms,
        serde_json::json!({ "operator_id": operator_id }),
    );
    store.append_event(session_id, &event)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_access_writes_playback_accessed_with_operator() {
        let store = ManifestStore::in_memory().unwrap();
        // `events.session_id` — внешний ключ на `sessions`, сессия должна
        // существовать в манифесте до записи события.
        store
            .insert_session(&SessionRecord::new(
                "sess-1",
                "/rec/sess-1",
                1_700_000_000_000,
                "station-1",
                "operator-7",
                44_100,
                1,
                16,
            ))
            .unwrap();
        record_access(&store, "sess-1", "op-7", 1_700_000_100_000).unwrap();

        let events = store.get_events("sess-1").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.kind, EventKind::PlaybackAccessed);
        assert_eq!(events[0].event.at_unix_ms, 1_700_000_100_000);
        assert_eq!(
            events[0].event.detail.as_ref().unwrap()["operator_id"],
            serde_json::json!("op-7")
        );
    }
}
