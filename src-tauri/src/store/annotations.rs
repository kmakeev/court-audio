//! Персист живой разметки в SQLite-манифест (этап 10 —
//! `promts/10_markers_realtime.md`, шаг 1).
//!
//! Таблица `annotations` — нормализованный append-only лог действий разметки
//! (модель — [`crate::integrity::annotations`]). Наполняется реконсиляцией из
//! write-ahead журнала ([`crate::store::reconcile`]); читается экспортом
//! ([`crate::store::export`]) и UI. `chain_link` вычислен при постановке —
//! здесь только хранение (пере-хеширование не делаем). Вставка идемпотентна
//! (`INSERT OR IGNORE` по `(session_id, seq)`), чтобы повторная реконсиляция
//! не плодила дублей.

use crate::integrity::annotations::{AnnotationAction, AnnotationRecord};

use super::manifest::ManifestStore;
use super::StoreError;

impl ManifestStore {
    /// Дописать действие разметки (идемпотентно по `(session_id, seq)`).
    pub fn append_annotation(
        &self,
        session_id: &str,
        rec: &AnnotationRecord,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT OR IGNORE INTO annotations (
                session_id, seq, action, target_id, category, role, comment,
                offset_samples, offset_ms, operator_id, at_unix_ms, chain_link
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            rusqlite::params![
                session_id,
                rec.seq as i64,
                rec.action.as_code(),
                rec.target_id,
                rec.category,
                rec.role,
                rec.comment,
                rec.offset_samples as i64,
                rec.offset_ms as i64,
                rec.operator_id,
                rec.at_unix_ms as i64,
                rec.chain_link,
            ],
        )?;
        Ok(())
    }

    /// Действия разметки сессии в порядке `seq`.
    pub fn get_annotations(&self, session_id: &str) -> Result<Vec<AnnotationRecord>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT seq, action, target_id, category, role, comment,
                    offset_samples, offset_ms, operator_id, at_unix_ms, chain_link
             FROM annotations WHERE session_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            let action_code: String = row.get(1)?;
            Ok((
                row.get::<_, i64>(0)? as u32,
                action_code,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)? as u64,
                row.get::<_, i64>(7)? as u64,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)? as u64,
                row.get::<_, String>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (seq, action_code, target_id, category, role, comment, os, oms, op, at, link) = r?;
            let action = AnnotationAction::from_code(&action_code).ok_or_else(|| {
                StoreError::Serde(format!("неизвестное действие разметки: {action_code}"))
            })?;
            out.push(AnnotationRecord {
                seq,
                action,
                target_id,
                category,
                role,
                comment,
                offset_samples: os,
                offset_ms: oms,
                operator_id: op,
                at_unix_ms: at,
                chain_link: link,
            });
        }
        Ok(out)
    }

    /// Удалить разметку сессии (используется ретеншном при purge локальной копии).
    pub fn delete_annotations(&self, session_id: &str) -> Result<(), StoreError> {
        self.conn()
            .execute("DELETE FROM annotations WHERE session_id = ?1", [session_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::annotations::build_annotation_chain;

    fn ann(seq: u32, action: AnnotationAction, target: &str, category: Option<&str>) -> AnnotationRecord {
        AnnotationRecord {
            seq,
            action,
            target_id: target.to_string(),
            category: category.map(|s| s.to_string()),
            role: None,
            comment: None,
            offset_samples: seq as u64 * 1000,
            offset_ms: seq as u64 * 20,
            operator_id: "op-7".to_string(),
            at_unix_ms: 1_700_000_000_000 + seq as u64,
            chain_link: String::new(),
        }
    }

    fn seed_session(store: &ManifestStore) {
        use crate::store::manifest::SessionRecord;
        store
            .insert_session(&SessionRecord::new(
                "s1", "/rec/s1", 1_700_000_000_000, "st", "op", 44_100, 1, 16,
            ))
            .unwrap();
    }

    #[test]
    fn append_and_get_in_order() {
        let store = ManifestStore::in_memory().unwrap();
        seed_session(&store);
        let mut recs = vec![
            ann(1, AnnotationAction::MarkerAdded, "m1", Some("Инцидент")),
            ann(2, AnnotationAction::RoleStarted, "s1", None),
        ];
        let chain = build_annotation_chain(&recs);
        for (r, link) in recs.iter_mut().zip(chain) {
            r.chain_link = link;
        }
        for r in &recs {
            store.append_annotation("s1", r).unwrap();
        }
        let back = store.get_annotations("s1").unwrap();
        assert_eq!(back, recs);
    }

    #[test]
    fn append_is_idempotent_by_seq() {
        let store = ManifestStore::in_memory().unwrap();
        seed_session(&store);
        let mut r = ann(1, AnnotationAction::MarkerAdded, "m1", Some("Перерыв"));
        r.chain_link = "link1".into();
        store.append_annotation("s1", &r).unwrap();
        // Повторная вставка того же seq не плодит дубль и не падает.
        store.append_annotation("s1", &r).unwrap();
        assert_eq!(store.get_annotations("s1").unwrap().len(), 1);
    }

    #[test]
    fn delete_removes_annotations() {
        let store = ManifestStore::in_memory().unwrap();
        seed_session(&store);
        let mut r = ann(1, AnnotationAction::MarkerAdded, "m1", Some("Прочее"));
        r.chain_link = "l".into();
        store.append_annotation("s1", &r).unwrap();
        store.delete_annotations("s1").unwrap();
        assert!(store.get_annotations("s1").unwrap().is_empty());
    }
}
