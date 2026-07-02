//! Живая разметка заседания: закладки и интервалы ролей (этап 10 —
//! `promts/10_markers_realtime.md`, шаги 1/3).
//!
//! Разметка — **append-only журнал действий**: постановка/правка/удаление метки
//! и начало/конец интервала роли — это отдельные записи ([`AnnotationRecord`]),
//! исходные не перезаписываются (правка фиксируется событием). Текущее состояние
//! (метки + интервалы) — свёртка журнала ([`fold`]). Каждая запись под
//! **хеш-цепочкой** (tamper-evident) — тем же приёмом, что сегменты
//! ([`crate::integrity::hash`]): порча любой записи ломает последующие звенья.
//!
//! Модуль **без I/O**: только модель, чистые вычисления оси/цепочки и свёртка.
//! Персист — write-ahead журнал (`recorder::journal`) → SQLite
//! (`store::annotations`) → манифест выгрузки (`store::export`). Смещения
//! семпл-точны относительно **общей оси сессии** (единый `started_at` + частота);
//! ASR/диаризацию здесь не дублируем — это W2.11 в `ex_system`.

use serde::{Deserialize, Serialize};

use super::hash;
use crate::recorder::multitrack::TrackMap;

/// Действие разметки. Сериализуется в `snake_case` — это и есть код `action`
/// в журнале, таблице `annotations` и экспортируемом манифесте.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationAction {
    /// Поставлена закладка (категория + опц. комментарий).
    MarkerAdded,
    /// Правка закладки (категория/комментарий) — новая запись, не перезапись.
    MarkerEdited,
    /// Удаление закладки (до завершения сессии).
    MarkerRemoved,
    /// Начало интервала «сейчас говорит <роль>».
    RoleStarted,
    /// Конец интервала роли.
    RoleEnded,
}

impl AnnotationAction {
    /// Стабильный строковый код (как в SQLite/журнале/манифесте).
    pub fn as_code(self) -> &'static str {
        match self {
            AnnotationAction::MarkerAdded => "marker_added",
            AnnotationAction::MarkerEdited => "marker_edited",
            AnnotationAction::MarkerRemoved => "marker_removed",
            AnnotationAction::RoleStarted => "role_started",
            AnnotationAction::RoleEnded => "role_ended",
        }
    }

    /// Разобрать код обратно (для чтения из SQLite-манифеста).
    pub fn from_code(code: &str) -> Option<Self> {
        let a = match code {
            "marker_added" => AnnotationAction::MarkerAdded,
            "marker_edited" => AnnotationAction::MarkerEdited,
            "marker_removed" => AnnotationAction::MarkerRemoved,
            "role_started" => AnnotationAction::RoleStarted,
            "role_ended" => AnnotationAction::RoleEnded,
            _ => return None,
        };
        Some(a)
    }
}

/// Одно действие разметки в журнале сессии. `target_id` стабилен между правками
/// (закладка/интервал адресуется им), `operator_id` — авторство (кто поставил),
/// `chain_link` — звено хеш-цепочки разметки (целостность/tamper-evidence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationRecord {
    /// Порядковый номер действия в рамках сессии (монотонный, с 1).
    pub seq: u32,
    pub action: AnnotationAction,
    /// Стабильный id закладки/интервала (не меняется при правке/удалении).
    pub target_id: String,
    /// Категория закладки (для `marker_*`; из справочника `markers.categories`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Роль интервала (для `role_*`; из справочника `audio.roles`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Свободный комментарий оператора (для закладок).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Семпл-точное смещение по оси сессии.
    pub offset_samples: u64,
    /// Смещение в миллисекундах (дубль для потребителей без частоты).
    pub offset_ms: u64,
    /// Автор действия (оператор). До экрана входа — из env (см. `ipc`).
    pub operator_id: String,
    /// Метка времени действия — мс от эпохи Unix.
    pub at_unix_ms: u64,
    /// Звено хеш-цепочки разметки на этой записи.
    pub chain_link: String,
}

/// Семпл-точное смещение действия по оси сессии: `(Δмс × частота) / 1000` с
/// округлением к ближайшему семплу. Ось — общий `started_at_unix_ms` сессии
/// (в многоканале дорожки стартуют синхронно). Часы монотонны в пределах
/// сессии — до старта `at < started` даёт 0 (saturating).
pub fn sample_offset(session_started_ms: u64, at_unix_ms: u64, sample_rate_hz: u32) -> u64 {
    let delta_ms = at_unix_ms.saturating_sub(session_started_ms) as u128;
    let rate = sample_rate_hz as u128;
    ((delta_ms * rate + 500) / 1000) as u64
}

/// Смещение действия в миллисекундах от старта сессии (saturating).
pub fn ms_offset(session_started_ms: u64, at_unix_ms: u64) -> u64 {
    at_unix_ms.saturating_sub(session_started_ms)
}

/// Роль активной дорожки для подстановки при разметке ролей (многоканал —
/// этап 09). `None`, если дорожки с таким `track_id` нет. Ручная корректировка
/// возможна — вызывающий передаёт роль явно (см. `ipc::marker_cmds`).
pub fn role_for_track(map: &TrackMap, track_id: u32) -> Option<String> {
    map.tracks
        .iter()
        .find(|t| t.track_id == track_id)
        .map(|t| t.role.clone())
}

/// Каноничный дайджест записи (sha256) — по всем полям **кроме** `chain_link`
/// и `seq` (порядок несёт цепочка, а не сам дайджест). Детерминированная строка:
/// изменение любого поля меняет дайджест, а значит и последующие звенья.
fn canonical_digest(rec: &AnnotationRecord) -> String {
    // Разделитель `\u{1f}` (unit separator) — не встречается в тексте категорий/
    // комментариев, поэтому поля не «склеиваются» неоднозначно.
    let canonical = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        rec.action.as_code(),
        rec.target_id,
        rec.category.as_deref().unwrap_or(""),
        rec.role.as_deref().unwrap_or(""),
        rec.comment.as_deref().unwrap_or(""),
        rec.offset_samples,
        rec.offset_ms,
        rec.operator_id,
        rec.at_unix_ms,
    );
    hash::sha256_bytes(canonical.as_bytes())
}

/// Звено цепочки для новой записи: `H(prev_link || H(canonical))` — как звено
/// сегментной цепочки ([`hash::chain_link`]), но по каноничному дайджесту записи.
pub fn next_link(prev_link: Option<&str>, rec: &AnnotationRecord) -> String {
    hash::chain_link(prev_link, &canonical_digest(rec))
}

/// Построить полную цепочку звеньев по упорядоченным записям (для проверки/
/// пересчёта). `chain[i]` соответствует записи `i`; финал — `chain.last()`.
pub fn build_annotation_chain(records: &[AnnotationRecord]) -> Vec<String> {
    let mut chain = Vec::with_capacity(records.len());
    let mut prev: Option<String> = None;
    for rec in records {
        let link = next_link(prev.as_deref(), rec);
        prev = Some(link.clone());
        chain.push(link);
    }
    chain
}

/// Верифицировать целостность лога разметки: пересчитать цепочку по каноничным
/// дайджестам записей и сверить с сохранёнными `chain_link`. Любая правка поля
/// (подмена категории/смещения/автора) даёт `false`.
pub fn verify_annotation_chain(records: &[AnnotationRecord]) -> bool {
    let recomputed = build_annotation_chain(records);
    recomputed
        .iter()
        .zip(records.iter())
        .all(|(link, rec)| *link == rec.chain_link)
        && recomputed.len() == records.len()
}

/// Финальное звено лога разметки (итог целостности), или `None` для пустого лога.
pub fn final_link(records: &[AnnotationRecord]) -> Option<String> {
    records.last().map(|r| r.chain_link.clone())
}

// ── Свёрнутое состояние (текущие метки/интервалы) ────────────────────────────

/// Закладка в текущем состоянии (после свёртки правок/удалений).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerState {
    pub id: String,
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub offset_samples: u64,
    pub offset_ms: u64,
    pub operator_id: String,
    pub at_unix_ms: u64,
}

/// Интервал роли в текущем состоянии. `end_*` = `None`, пока интервал открыт.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSpanState {
    pub id: String,
    pub role: String,
    pub start_offset_samples: u64,
    pub start_offset_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_offset_samples: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_offset_ms: Option<u64>,
    pub operator_id: String,
    pub at_unix_ms: u64,
}

/// Свёрнутый снимок разметки: текущие закладки и интервалы ролей (по возрастанию
/// смещения).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationSnapshot {
    pub markers: Vec<MarkerState>,
    pub role_spans: Vec<RoleSpanState>,
}

/// Свернуть журнал действий в текущее состояние. Порядок применения — по журналу
/// (правки/удаления после добавления). Метки без `MarkerRemoved` остаются;
/// интервалы получают конец по `RoleEnded`.
pub fn fold(records: &[AnnotationRecord]) -> AnnotationSnapshot {
    use std::collections::HashMap;

    let mut markers: HashMap<String, MarkerState> = HashMap::new();
    let mut spans: HashMap<String, RoleSpanState> = HashMap::new();

    for rec in records {
        match rec.action {
            AnnotationAction::MarkerAdded => {
                markers.insert(
                    rec.target_id.clone(),
                    MarkerState {
                        id: rec.target_id.clone(),
                        category: rec.category.clone().unwrap_or_default(),
                        comment: rec.comment.clone(),
                        offset_samples: rec.offset_samples,
                        offset_ms: rec.offset_ms,
                        operator_id: rec.operator_id.clone(),
                        at_unix_ms: rec.at_unix_ms,
                    },
                );
            }
            AnnotationAction::MarkerEdited => {
                if let Some(m) = markers.get_mut(&rec.target_id) {
                    // Правим категорию/комментарий; смещение закладки сохраняем.
                    if let Some(cat) = &rec.category {
                        m.category = cat.clone();
                    }
                    m.comment = rec.comment.clone();
                }
            }
            AnnotationAction::MarkerRemoved => {
                markers.remove(&rec.target_id);
            }
            AnnotationAction::RoleStarted => {
                spans.insert(
                    rec.target_id.clone(),
                    RoleSpanState {
                        id: rec.target_id.clone(),
                        role: rec.role.clone().unwrap_or_default(),
                        start_offset_samples: rec.offset_samples,
                        start_offset_ms: rec.offset_ms,
                        end_offset_samples: None,
                        end_offset_ms: None,
                        operator_id: rec.operator_id.clone(),
                        at_unix_ms: rec.at_unix_ms,
                    },
                );
            }
            AnnotationAction::RoleEnded => {
                if let Some(s) = spans.get_mut(&rec.target_id) {
                    s.end_offset_samples = Some(rec.offset_samples);
                    s.end_offset_ms = Some(rec.offset_ms);
                }
            }
        }
    }

    let mut markers: Vec<MarkerState> = markers.into_values().collect();
    markers.sort_by_key(|m| (m.offset_samples, m.id.clone()));
    let mut role_spans: Vec<RoleSpanState> = spans.into_values().collect();
    role_spans.sort_by_key(|s| (s.start_offset_samples, s.id.clone()));

    AnnotationSnapshot {
        markers,
        role_spans,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::multitrack::track_map_from_resolved;
    use crate::audio::tracks::ResolvedTrack;

    fn rec(
        seq: u32,
        action: AnnotationAction,
        target: &str,
        category: Option<&str>,
        role: Option<&str>,
        offset_samples: u64,
        offset_ms: u64,
    ) -> AnnotationRecord {
        AnnotationRecord {
            seq,
            action,
            target_id: target.to_string(),
            category: category.map(|s| s.to_string()),
            role: role.map(|s| s.to_string()),
            comment: None,
            offset_samples,
            offset_ms,
            operator_id: "op-7".to_string(),
            at_unix_ms: 1_700_000_000_000 + offset_ms,
            chain_link: String::new(),
        }
    }

    /// Заполнить `chain_link` записей корректной цепочкой (как при постановке).
    fn chained(mut records: Vec<AnnotationRecord>) -> Vec<AnnotationRecord> {
        let chain = build_annotation_chain(&records);
        for (r, link) in records.iter_mut().zip(chain) {
            r.chain_link = link;
        }
        records
    }

    #[test]
    fn action_code_roundtrips() {
        for a in [
            AnnotationAction::MarkerAdded,
            AnnotationAction::MarkerEdited,
            AnnotationAction::MarkerRemoved,
            AnnotationAction::RoleStarted,
            AnnotationAction::RoleEnded,
        ] {
            assert_eq!(AnnotationAction::from_code(a.as_code()), Some(a));
        }
        assert_eq!(AnnotationAction::from_code("garbage"), None);
    }

    #[test]
    fn sample_offset_is_sample_accurate() {
        // 0.5 c при 44100 Гц = 22050 семплов.
        assert_eq!(sample_offset(1_000, 1_500, 44_100), 22_050);
        // Момент старта — нулевое смещение.
        assert_eq!(sample_offset(1_000, 1_000, 44_100), 0);
        // До старта (часы разошлись) — не уходим в переполнение, 0.
        assert_eq!(sample_offset(2_000, 1_000, 44_100), 0);
        // Округление к ближайшему семплу: 1 мс при 44100 = 44.1 → 44.
        assert_eq!(sample_offset(0, 1, 44_100), 44);
        // 48 кГц, ровно секунда.
        assert_eq!(sample_offset(0, 1_000, 48_000), 48_000);
        assert_eq!(ms_offset(1_000, 1_500), 500);
        assert_eq!(ms_offset(2_000, 1_000), 0);
    }

    #[test]
    fn chain_builds_and_verifies() {
        let records = chained(vec![
            rec(1, AnnotationAction::MarkerAdded, "m1", Some("Инцидент"), None, 100, 2),
            rec(2, AnnotationAction::RoleStarted, "s1", None, Some("judge"), 200, 4),
            rec(3, AnnotationAction::RoleEnded, "s1", None, None, 300, 6),
        ]);
        assert!(verify_annotation_chain(&records));
        assert_eq!(final_link(&records), records.last().map(|r| r.chain_link.clone()));
    }

    #[test]
    fn tampering_breaks_chain() {
        let mut records = chained(vec![
            rec(1, AnnotationAction::MarkerAdded, "m1", Some("Инцидент"), None, 100, 2),
            rec(2, AnnotationAction::MarkerAdded, "m2", Some("Перерыв"), None, 200, 4),
        ]);
        assert!(verify_annotation_chain(&records));
        // Подмена категории первой метки (chain_link не пересчитан).
        records[0].category = Some("Прочее".to_string());
        assert!(!verify_annotation_chain(&records));
    }

    #[test]
    fn fold_applies_edit_remove_and_role_end() {
        let records = chained(vec![
            rec(1, AnnotationAction::MarkerAdded, "m1", Some("Инцидент"), None, 100, 2),
            rec(2, AnnotationAction::MarkerAdded, "m2", Some("Перерыв"), None, 400, 8),
            // Правка m1: категория меняется, смещение сохраняется.
            rec(3, AnnotationAction::MarkerEdited, "m1", Some("Прочее"), None, 0, 0),
            // Удаление m2.
            rec(4, AnnotationAction::MarkerRemoved, "m2", None, None, 0, 0),
            rec(5, AnnotationAction::RoleStarted, "s1", None, Some("judge"), 150, 3),
            rec(6, AnnotationAction::RoleEnded, "s1", None, None, 350, 7),
        ]);
        let snap = fold(&records);
        assert_eq!(snap.markers.len(), 1);
        assert_eq!(snap.markers[0].id, "m1");
        assert_eq!(snap.markers[0].category, "Прочее");
        assert_eq!(snap.markers[0].offset_samples, 100); // смещение не сбилось
        assert_eq!(snap.role_spans.len(), 1);
        assert_eq!(snap.role_spans[0].role, "judge");
        assert_eq!(snap.role_spans[0].end_offset_samples, Some(350));
    }

    #[test]
    fn open_role_span_has_no_end() {
        let records = chained(vec![rec(
            1,
            AnnotationAction::RoleStarted,
            "s1",
            None,
            Some("witness"),
            10,
            1,
        )]);
        let snap = fold(&records);
        assert_eq!(snap.role_spans.len(), 1);
        assert_eq!(snap.role_spans[0].end_offset_samples, None);
    }

    #[test]
    fn role_for_track_suggests_and_allows_override() {
        let map = track_map_from_resolved(&[
            ResolvedTrack {
                track_id: 0,
                device: None,
                channel_index: 0,
                role: "judge".into(),
                label: "judge".into(),
            },
            ResolvedTrack {
                track_id: 1,
                device: None,
                channel_index: 1,
                role: "defense".into(),
                label: "defense".into(),
            },
        ]);
        // Подстановка роли активной дорожки.
        assert_eq!(role_for_track(&map, 1).as_deref(), Some("defense"));
        assert_eq!(role_for_track(&map, 0).as_deref(), Some("judge"));
        // Нет такой дорожки — None (вызывающий укажет роль вручную).
        assert_eq!(role_for_track(&map, 5), None);
    }
}
