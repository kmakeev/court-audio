//! Модель привязки записи к делу (этап 05 — `promts/05_case_binding_offline.md`,
//! «Модель привязки» / deliverable 4).
//!
//! Привязка имеет два состояния:
//! - **Resolved** — оператор выбрал дело из кэша докета; в манифест идёт
//!   `adjudication_id` (сервер сразу связывает с `Adjudication`).
//! - **Manual** (pending) — дела нет в кэше / станция оффлайн без кэша; оператор
//!   ввёл № (и опц. ФИО) вручную; финальное связывание делает сервер/оператор
//!   позже (`06`/`07`/W2.11).
//!
//! Хранится **JSON-строкой** в существующей TEXT-колонке `sessions.adjudication_ref`
//! (этап 03): схему БД и экспорт манифеста ([`super::export`]) не меняем — строка
//! проходит насквозь, сервер `07` парсит этот JSON. Запись **никогда не
//! блокируется** отсутствием привязки — её можно проставить/уточнить позже.

use serde::{Deserialize, Serialize};

use super::StoreError;

/// Состояние привязки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    /// Выбрано дело из кэша докета — есть `adjudication_id`.
    Resolved,
    /// Ручной ввод — сырой № (+опц. ФИО), связывание отложено серверу.
    Manual,
}

/// Привязка записи к делу. Сериализуется в JSON и кладётся в
/// `sessions.adjudication_ref`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjudicationRef {
    pub kind: BindingKind,
    /// Идентификатор `Adjudication` на сервере (только для `Resolved`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adjudication_id: Option<String>,
    /// Сырой № дела (для `Manual`; информативно дублируется и для `Resolved`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_number: Option<String>,
    /// Сырые ФИО сторон (опционально).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_fio: Option<String>,
}

impl AdjudicationRef {
    /// Привязка к делу из кэша. `raw_number`/`raw_fio` — опц. человекочитаемая
    /// подпись (для UI/манифеста), не влияет на связывание.
    pub fn resolved(
        adjudication_id: impl Into<String>,
        raw_number: Option<String>,
        raw_fio: Option<String>,
    ) -> Self {
        Self {
            kind: BindingKind::Resolved,
            adjudication_id: Some(adjudication_id.into()),
            raw_number: raw_number.filter(|s| !s.trim().is_empty()),
            raw_fio: raw_fio.filter(|s| !s.trim().is_empty()),
        }
    }

    /// Ручная (pending) привязка: обязателен непустой №; ФИО — опционально.
    pub fn manual(
        raw_number: impl Into<String>,
        raw_fio: Option<String>,
    ) -> Result<Self, StoreError> {
        let raw_number = raw_number.into();
        if raw_number.trim().is_empty() {
            return Err(StoreError::Serde(
                "ручная привязка требует непустой № дела".into(),
            ));
        }
        Ok(Self {
            kind: BindingKind::Manual,
            adjudication_id: None,
            raw_number: Some(raw_number),
            raw_fio: raw_fio.filter(|s| !s.trim().is_empty()),
        })
    }

    /// Валидация согласованности состояния (страхует данные «снаружи», напр. из IPC):
    /// `Resolved` обязан иметь `adjudication_id`; `Manual` — `raw_number`.
    pub fn validate(&self) -> Result<(), StoreError> {
        match self.kind {
            BindingKind::Resolved => {
                if self
                    .adjudication_id
                    .as_ref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true)
                {
                    return Err(StoreError::Serde(
                        "resolved-привязка требует adjudication_id".into(),
                    ));
                }
            }
            BindingKind::Manual => {
                if self
                    .raw_number
                    .as_ref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true)
                {
                    return Err(StoreError::Serde(
                        "manual-привязка требует raw_number".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Сериализовать в JSON для колонки манифеста.
    pub fn to_json(&self) -> Result<String, StoreError> {
        Ok(serde_json::to_string(self)?)
    }

    /// Разобрать из JSON-строки колонки манифеста.
    pub fn from_json(s: &str) -> Result<Self, StoreError> {
        Ok(serde_json::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_roundtrips_json() {
        let r = AdjudicationRef::resolved(
            "adj-42",
            Some("№ 1-123/2026".into()),
            Some("Иванов И.И.".into()),
        );
        r.validate().unwrap();
        let json = r.to_json().unwrap();
        assert!(json.contains("\"resolved\""));
        assert!(json.contains("adj-42"));
        let back = AdjudicationRef::from_json(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn manual_roundtrips_json_and_requires_number() {
        let r = AdjudicationRef::manual("№ 2-7/2026", None).unwrap();
        assert_eq!(r.kind, BindingKind::Manual);
        assert!(r.adjudication_id.is_none());
        r.validate().unwrap();
        let back = AdjudicationRef::from_json(&r.to_json().unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn empty_manual_number_is_rejected() {
        assert!(matches!(
            AdjudicationRef::manual("   ", None),
            Err(StoreError::Serde(_))
        ));
    }

    #[test]
    fn validate_catches_inconsistent_external_input() {
        // Resolved без id — невалиден (имитируем данные из IPC, минуя конструктор).
        let bad = AdjudicationRef {
            kind: BindingKind::Resolved,
            adjudication_id: None,
            raw_number: Some("№ 1".into()),
            raw_fio: None,
        };
        assert!(bad.validate().is_err());
        // Manual без номера — невалиден.
        let bad2 = AdjudicationRef {
            kind: BindingKind::Manual,
            adjudication_id: None,
            raw_number: None,
            raw_fio: Some("Петров".into()),
        };
        assert!(bad2.validate().is_err());
    }

    #[test]
    fn blank_optional_fields_dropped() {
        let r = AdjudicationRef::resolved("adj-1", Some("  ".into()), Some("".into()));
        assert!(r.raw_number.is_none());
        assert!(r.raw_fio.is_none());
        // skip_serializing_if прячет None — JSON компактный.
        let json = r.to_json().unwrap();
        assert!(!json.contains("raw_number"));
        assert!(!json.contains("raw_fio"));
    }
}
