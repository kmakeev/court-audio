//! Станционный журнал изменений настроек (этап 10.4 —
//! `promts/10_4_settings_roles.md`, deliverable 3).
//!
//! Каждое сохранение настроек — событие журнала: **кто** (`actor_operator_id`),
//! **когда** (`at_unix_ms`), **что** (`changes`: поле-уровневый diff
//! старое→новое, без секретов), опасное ли изменение (`dangerous`), источник
//! (`source`: ручное сохранение / импорт профиля). Хранится в станционной
//! таблице `settings_audit` (SQLite-манифест) — **не** в `events`: та привязана
//! FK к сессиям, а настройки станционные (вне сессии). Зеркалит паттерн аудита
//! экспорта ([`crate::export::audit`]): тонкий помощник поверх
//! [`ManifestStore`]. Пишется только при `Settings.integrity.event_log`.

use serde::{Deserialize, Serialize};

use super::manifest::ManifestStore;
use super::StoreError;

/// Одно изменённое поле настроек: dotted-путь + старое/новое значения. Значения —
/// `serde_json::Value` (как в конфиге); секретов в `Settings` нет.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldChange {
    pub path: String,
    pub old: serde_json::Value,
    pub new: serde_json::Value,
}

/// Источник изменения настроек (для журнала).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSource {
    /// Ручное сохранение на экране настроек/администрирования.
    Manual,
    /// Импорт профиля станции.
    Import,
}

impl ChangeSource {
    pub fn as_code(self) -> &'static str {
        match self {
            ChangeSource::Manual => "manual",
            ChangeSource::Import => "import",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "manual" => Some(ChangeSource::Manual),
            "import" => Some(ChangeSource::Import),
            _ => None,
        }
    }
}

/// Событие изменения настроек (запись в журнал).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsChange {
    pub at_unix_ms: u64,
    /// Кто менял (`operator_id` вошедшего оператора; пусто — вход не требовался).
    pub actor_operator_id: String,
    pub source: ChangeSource,
    /// Затронуто ли хотя бы одно «опасное» изменение (сервер/шифрование/ретеншн).
    pub dangerous: bool,
    /// Поле-уровневый diff (старое→новое, без секретов).
    pub changes: Vec<FieldChange>,
}

/// Прочитанная запись журнала с монотонным `seq` (для UI/диагностики).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsAuditRecord {
    pub seq: i64,
    #[serde(flatten)]
    pub change: SettingsChange,
}

/// Записать событие изменения настроек. Тонкий помощник (зеркало
/// [`crate::export::audit::record_export`]) — сборку `change` делает вызывающий
/// (`ipc`), гейт `integrity.event_log` — тоже.
pub fn record(store: &ManifestStore, change: &SettingsChange) -> Result<i64, StoreError> {
    store.append_settings_change(change)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(at: u64, actor: &str, dangerous: bool) -> SettingsChange {
        SettingsChange {
            at_unix_ms: at,
            actor_operator_id: actor.to_string(),
            source: ChangeSource::Manual,
            dangerous,
            changes: vec![FieldChange {
                path: "sync.server_base_url".into(),
                old: serde_json::Value::Null,
                new: serde_json::json!("https://ex.example"),
            }],
        }
    }

    #[test]
    fn source_code_roundtrips() {
        for s in [ChangeSource::Manual, ChangeSource::Import] {
            assert_eq!(ChangeSource::from_code(s.as_code()), Some(s));
        }
        assert_eq!(ChangeSource::from_code("garbage"), None);
    }

    #[test]
    fn append_and_list_in_order() {
        let store = ManifestStore::in_memory().unwrap();
        record(&store, &change(1_000, "op-1", false)).unwrap();
        record(&store, &change(2_000, "op-2", true)).unwrap();

        let all = store.list_settings_audit(100).unwrap();
        assert_eq!(all.len(), 2);
        // Возврат — новейшие сверху (для журнала UI).
        assert_eq!(all[0].change.at_unix_ms, 2_000);
        assert_eq!(all[0].change.actor_operator_id, "op-2");
        assert!(all[0].change.dangerous);
        assert_eq!(all[1].change.at_unix_ms, 1_000);
        assert!(all[0].seq > all[1].seq);
        // Поле-уровневый diff сохранён.
        assert_eq!(all[0].change.changes[0].path, "sync.server_base_url");
    }

    #[test]
    fn list_respects_limit() {
        let store = ManifestStore::in_memory().unwrap();
        for i in 0..5 {
            record(&store, &change(i, "op", false)).unwrap();
        }
        assert_eq!(store.list_settings_audit(3).unwrap().len(), 3);
    }
}
