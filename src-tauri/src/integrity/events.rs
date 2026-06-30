//! Структурированный журнал событий записи (этап 03 —
//! `promts/03_store_integrity.md`, deliverable 3).
//!
//! Часть записи целостности: значимые-для-целостности события сессии
//! (старт/пауза/обрыв устройства/восстановление/стоп/ротация) с метками времени.
//! Зеркало значимых вариантов `recorder::journal::JournalRecord` (этап 02): тот
//! журнал — аварийно-устойчивый write-ahead на диске, а здесь — нормализованная
//! модель для SQLite-манифеста (`store::manifest`) и экспорта (`store::export`).
//! Подробные диагностические логи остаются локально и сюда не попадают
//! (решение «Решено» в промте). Включается флагом `Settings.integrity.event_log`.

use serde::{Deserialize, Serialize};

/// Значимое событие записи с меткой времени (Unix-миллисекунды).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingEvent {
    /// Тип события (стабильный machine-readable код, `snake_case`).
    pub kind: EventKind,
    /// Метка времени события — мс от эпохи Unix.
    pub at_unix_ms: u64,
    /// Дополнительный контекст (причина паузы, свободное место и т.п.) — JSON-
    /// объект или `null`. Не содержит ПДн.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

/// Тип значимого события. Сериализуется в `snake_case` — это и есть код `kind`
/// в таблице `events` и в экспортируемом манифесте.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Старт сессии записи.
    SessionStarted,
    /// Ротация сегмента (закрыт сегмент, открыт следующий).
    SegmentRotated,
    /// Пауза (детали — причина: operator/device_lost).
    Paused,
    /// Возобновление после паузы.
    Resumed,
    /// Обрыв/пропажа устройства.
    DeviceLost,
    /// Возврат устройства.
    DeviceBack,
    /// Сессия помечена восстановленной после сбоя.
    Recovered,
    /// Корректное завершение сессии.
    Stopped,
}

impl EventKind {
    /// Стабильный строковый код (как в SQLite/манифесте). Совпадает с
    /// serde-представлением.
    pub fn as_code(self) -> &'static str {
        match self {
            EventKind::SessionStarted => "session_started",
            EventKind::SegmentRotated => "segment_rotated",
            EventKind::Paused => "paused",
            EventKind::Resumed => "resumed",
            EventKind::DeviceLost => "device_lost",
            EventKind::DeviceBack => "device_back",
            EventKind::Recovered => "recovered",
            EventKind::Stopped => "stopped",
        }
    }

    /// Разобрать код обратно (для чтения из SQLite-манифеста).
    pub fn from_code(code: &str) -> Option<Self> {
        let kind = match code {
            "session_started" => EventKind::SessionStarted,
            "segment_rotated" => EventKind::SegmentRotated,
            "paused" => EventKind::Paused,
            "resumed" => EventKind::Resumed,
            "device_lost" => EventKind::DeviceLost,
            "device_back" => EventKind::DeviceBack,
            "recovered" => EventKind::Recovered,
            "stopped" => EventKind::Stopped,
            _ => return None,
        };
        Some(kind)
    }
}

impl RecordingEvent {
    /// Событие без дополнительного контекста.
    pub fn new(kind: EventKind, at_unix_ms: u64) -> Self {
        Self {
            kind,
            at_unix_ms,
            detail: None,
        }
    }

    /// Событие с контекстом.
    pub fn with_detail(kind: EventKind, at_unix_ms: u64, detail: serde_json::Value) -> Self {
        Self {
            kind,
            at_unix_ms,
            detail: Some(detail),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_code_roundtrips() {
        for kind in [
            EventKind::SessionStarted,
            EventKind::SegmentRotated,
            EventKind::Paused,
            EventKind::Resumed,
            EventKind::DeviceLost,
            EventKind::DeviceBack,
            EventKind::Recovered,
            EventKind::Stopped,
        ] {
            assert_eq!(EventKind::from_code(kind.as_code()), Some(kind));
        }
        assert_eq!(EventKind::from_code("garbage"), None);
    }

    #[test]
    fn serde_uses_snake_case_code() {
        let ev = RecordingEvent::with_detail(
            EventKind::Paused,
            1_700_000_000_000,
            serde_json::json!({ "reason": "device_lost" }),
        );
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"kind\":\"paused\""));
        assert!(json.contains("\"reason\":\"device_lost\""));
        let back: RecordingEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn detail_omitted_when_absent() {
        let ev = RecordingEvent::new(EventKind::Stopped, 42);
        let json = serde_json::to_string(&ev).unwrap();
        assert!(!json.contains("detail"));
    }
}
