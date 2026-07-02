//! Аудит экспорта (этап 10.2, шаг 5). Зеркалит [`crate::player::audit`]:
//! экспорт выводит ПДн из зашифрованного хранилища — журналируется **всегда**,
//! и успех, и отказ политикой администратора (`detail.result`).

use crate::integrity::events::{EventKind, RecordingEvent};
use crate::store::manifest::ManifestStore;
use crate::store::StoreError;

/// Записать событие экспорта. `detail` — свободная структура (состав/формат/
/// назначение/оператор/результат), собирается вызывающим (`ipc::export_cmds`).
pub fn record_export(
    store: &ManifestStore,
    session_id: &str,
    at_unix_ms: u64,
    detail: serde_json::Value,
) -> Result<(), StoreError> {
    let event = RecordingEvent::with_detail(EventKind::ExportCreated, at_unix_ms, detail);
    store.append_event(session_id, &event)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::manifest::SessionRecord;

    fn seeded_store() -> ManifestStore {
        let store = ManifestStore::in_memory().unwrap();
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
        store
    }

    #[test]
    fn record_export_writes_export_created_with_success_detail() {
        let store = seeded_store();
        record_export(
            &store,
            "sess-1",
            1_700_000_100_000,
            serde_json::json!({"operator_id": "op-7", "result": "ok", "format": "wav_pcm"}),
        )
        .unwrap();

        let events = store.get_events("sess-1").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.kind, EventKind::ExportCreated);
        assert_eq!(events[0].event.detail.as_ref().unwrap()["result"], "ok");
    }

    #[test]
    fn record_export_writes_export_created_with_denied_detail() {
        let store = seeded_store();
        record_export(
            &store,
            "sess-1",
            1_700_000_100_000,
            serde_json::json!({"operator_id": "op-7", "result": "denied", "reason": "policy_forbidden"}),
        )
        .unwrap();

        let events = store.get_events("sess-1").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.detail.as_ref().unwrap()["result"], "denied");
    }
}
