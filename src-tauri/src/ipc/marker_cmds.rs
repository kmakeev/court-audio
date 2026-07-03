//! Tauri-команды живой разметки заседания (этап 10 —
//! `promts/10_markers_realtime.md`, шаг 2).
//!
//! Оператор ставит закладки (категория + опц. комментарий) и отмечает интервалы
//! ролей по ходу записи. Команды идут **вне аудио-потока** (как пауза/стоп),
//! поэтому не мешают захвату (deliverable: «приоритет записи сохранён»). Каждое
//! действие: считает семпл-точное смещение по оси активной сессии, звено
//! хеш-цепочки разметки и пишет запись в write-ahead журнал (fsync) — так метки
//! переживают рестарт и tamper-evident (`integrity::annotations`).
//!
//! Слой IPC — единственное место с Tauri-зависимостью; модель/цепочка/свёртка
//! живут в ядре (`integrity::annotations`) и тестируются без Tauri.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, State};

use crate::integrity::annotations::{
    self, AnnotationAction, AnnotationRecord, AnnotationSnapshot,
};
use crate::ipc::audio_cmds::CaptureState;
use crate::ipc::load_settings;
use crate::recorder::journal::{Journal, JournalRecord};
use crate::reliability::watchdog::now_unix_ms;

/// Рантайм-состояние разметки активной сессии: write-ahead журнал (общий для
/// сегментов и разметки в одноканале), счётчик действий, последнее звено цепочки
/// и in-memory лог для быстрого показа списка (без ре-реплея журнала).
pub struct AnnotationState {
    journal: Arc<Mutex<Journal>>,
    seq: u32,
    prev_link: Option<String>,
    entries: Vec<AnnotationRecord>,
}

impl AnnotationState {
    /// Новое состояние поверх журнала каталога сессии.
    pub fn new(journal: Arc<Mutex<Journal>>) -> Self {
        Self {
            journal,
            seq: 0,
            prev_link: None,
            entries: Vec::new(),
        }
    }

    /// Зафиксировать действие: присвоить `seq`, посчитать звено цепочки, записать
    /// в журнал (fsync) и в in-memory лог. Порядок: сначала журнал (write-ahead),
    /// затем сдвиг состояния — при сбое записи состояние не «уедет».
    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        action: AnnotationAction,
        target_id: String,
        category: Option<String>,
        role: Option<String>,
        comment: Option<String>,
        offset_samples: u64,
        offset_ms: u64,
        operator_id: String,
        at_unix_ms: u64,
    ) -> Result<(), String> {
        let seq = self.seq + 1;
        let mut rec = AnnotationRecord {
            seq,
            action,
            target_id,
            category,
            role,
            comment,
            offset_samples,
            offset_ms,
            operator_id,
            at_unix_ms,
            chain_link: String::new(),
        };
        rec.chain_link = annotations::next_link(self.prev_link.as_deref(), &rec);
        {
            let mut j = self
                .journal
                .lock()
                .map_err(|_| "журнал разметки повреждён".to_string())?;
            j.append(&JournalRecord::Annotation(rec.clone()))
                .map_err(|e| e.to_string())?;
        }
        self.prev_link = Some(rec.chain_link.clone());
        self.seq = seq;
        self.entries.push(rec);
        Ok(())
    }

    /// Текущее свёрнутое состояние (метки/интервалы) для UI.
    pub fn snapshot(&self) -> AnnotationSnapshot {
        annotations::fold(&self.entries)
    }

    /// Завершить все открытые интервалы ролей в указанный момент (этап 10.6):
    /// новый говорящий автоматически закрывает предыдущего, без ручного
    /// «Завершить». Каждое закрытие — отдельное журналируемое действие `RoleEnded`
    /// под хеш-цепочкой (как ручное завершение). Возвращает id закрытых интервалов.
    pub fn end_open_role_spans(
        &mut self,
        offset_samples: u64,
        offset_ms: u64,
        operator: &str,
        at_unix_ms: u64,
    ) -> Result<Vec<String>, String> {
        let open: Vec<String> = self
            .snapshot()
            .role_spans
            .into_iter()
            .filter(|s| s.end_offset_ms.is_none())
            .map(|s| s.id)
            .collect();
        for id in &open {
            self.record(
                AnnotationAction::RoleEnded,
                id.clone(),
                None,
                None,
                None,
                offset_samples,
                offset_ms,
                operator.to_string(),
                at_unix_ms,
            )?;
        }
        Ok(open)
    }
}

/// Сгенерировать стабильный id закладки/интервала: префикс + время + случайный
/// хвост (без внешних зависимостей на UUID; коллизии практически исключены).
fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}-{:08x}", now_unix_ms(), rand::random::<u32>())
}

/// Проверить, что значение есть в справочнике (категории/роли — из реестра).
fn ensure_in(dictionary: &[String], value: &str, what: &str) -> Result<(), String> {
    if dictionary.iter().any(|v| v == value) {
        Ok(())
    } else {
        Err(format!("{what} «{value}» вне справочника настроек"))
    }
}

/// Выполнить действие над разметкой активной сессии и вернуть свёрнутый снимок.
/// Смещение считается от оси активной сессии на момент действия.
fn with_active<F>(state: &State<'_, CaptureState>, act: F) -> Result<AnnotationSnapshot, String>
where
    F: FnOnce(&mut AnnotationState, u64, u64, &str) -> Result<(), String>,
{
    let guard = state
        .0
        .lock()
        .map_err(|_| "состояние захвата повреждено".to_string())?;
    let active = guard
        .as_ref()
        .ok_or_else(|| "запись не запущена — разметка недоступна".to_string())?;
    let now = now_unix_ms();
    let offset_samples =
        annotations::sample_offset(active.started_at_unix_ms, now, active.sample_rate_hz);
    let offset_ms = annotations::ms_offset(active.started_at_unix_ms, now);
    let operator = active.operator_id.clone();
    let mut ann = active
        .annotations
        .lock()
        .map_err(|_| "состояние разметки повреждено".to_string())?;
    act(&mut ann, offset_samples, offset_ms, &operator)?;
    Ok(ann.snapshot())
}

/// Поставить закладку в текущий момент записи (категория из справочника +
/// опц. комментарий). Смещение семпл-точно относительно оси сессии.
#[tauri::command]
pub fn add_marker(
    app: AppHandle,
    state: State<'_, CaptureState>,
    category: String,
    comment: Option<String>,
) -> Result<AnnotationSnapshot, String> {
    let settings = load_settings(&app)?;
    ensure_in(&settings.markers.categories, &category, "категория закладки")?;
    let comment = comment.filter(|c| !c.trim().is_empty());
    with_active(&state, |ann, offset_samples, offset_ms, operator| {
        ann.record(
            AnnotationAction::MarkerAdded,
            new_id("m"),
            Some(category.clone()),
            None,
            comment.clone(),
            offset_samples,
            offset_ms,
            operator.to_string(),
            now_unix_ms(),
        )
    })
}

/// Изменить закладку (категория/комментарий) — фиксируется отдельным действием
/// (не молчаливая перезапись); смещение закладки сохраняется.
#[tauri::command]
pub fn edit_marker(
    app: AppHandle,
    state: State<'_, CaptureState>,
    target_id: String,
    category: String,
    comment: Option<String>,
) -> Result<AnnotationSnapshot, String> {
    let settings = load_settings(&app)?;
    ensure_in(&settings.markers.categories, &category, "категория закладки")?;
    let comment = comment.filter(|c| !c.trim().is_empty());
    with_active(&state, |ann, _os, _oms, operator| {
        ann.record(
            AnnotationAction::MarkerEdited,
            target_id.clone(),
            Some(category.clone()),
            None,
            comment.clone(),
            0,
            0,
            operator.to_string(),
            now_unix_ms(),
        )
    })
}

/// Удалить закладку (до завершения сессии) — фиксируется действием `MarkerRemoved`.
#[tauri::command]
pub fn remove_marker(
    state: State<'_, CaptureState>,
    target_id: String,
) -> Result<AnnotationSnapshot, String> {
    with_active(&state, |ann, _os, _oms, operator| {
        ann.record(
            AnnotationAction::MarkerRemoved,
            target_id.clone(),
            None,
            None,
            None,
            0,
            0,
            operator.to_string(),
            now_unix_ms(),
        )
    })
}

/// Начать интервал «сейчас говорит <роль>». Роль берётся из аргумента, либо
/// подставляется из активной дорожки (`track_id`, многоканал — этап 09); ручная
/// корректировка = явный `role`. Роль валидируется по справочнику `audio.roles`.
#[tauri::command]
pub fn start_role_span(
    app: AppHandle,
    state: State<'_, CaptureState>,
    role: Option<String>,
    track_id: Option<u32>,
) -> Result<AnnotationSnapshot, String> {
    let settings = load_settings(&app)?;

    // Резолвим роль: явная имеет приоритет; иначе — из карты дорожек по track_id.
    let resolved_role = match role.filter(|r| !r.trim().is_empty()) {
        Some(r) => r,
        None => {
            let guard = state
                .0
                .lock()
                .map_err(|_| "состояние захвата повреждено".to_string())?;
            let active = guard
                .as_ref()
                .ok_or_else(|| "запись не запущена — разметка недоступна".to_string())?;
            let tid = track_id
                .ok_or_else(|| "роль не задана и не выведена из дорожки".to_string())?;
            active
                .track_map
                .as_ref()
                .and_then(|m| annotations::role_for_track(m, tid))
                .ok_or_else(|| format!("роль дорожки {tid} не определена"))?
        }
    };
    ensure_in(&settings.audio.roles, &resolved_role, "роль")?;

    with_active(&state, |ann, offset_samples, offset_ms, operator| {
        // Новый говорящий автоматически завершает предыдущего (этап 10.6): не нужно
        // жать «Завершить», если уже отмечен следующий. Закрытие — в тот же момент.
        ann.end_open_role_spans(offset_samples, offset_ms, operator, now_unix_ms())?;
        ann.record(
            AnnotationAction::RoleStarted,
            new_id("r"),
            None,
            Some(resolved_role.clone()),
            None,
            offset_samples,
            offset_ms,
            operator.to_string(),
            now_unix_ms(),
        )
    })
}

/// Завершить интервал роли в текущий момент записи.
#[tauri::command]
pub fn end_role_span(
    state: State<'_, CaptureState>,
    target_id: String,
) -> Result<AnnotationSnapshot, String> {
    with_active(&state, |ann, offset_samples, offset_ms, operator| {
        ann.record(
            AnnotationAction::RoleEnded,
            target_id.clone(),
            None,
            None,
            None,
            offset_samples,
            offset_ms,
            operator.to_string(),
            now_unix_ms(),
        )
    })
}

/// Текущая разметка активной сессии (свёрнутые метки/интервалы) для UI.
#[tauri::command]
pub fn list_annotations(state: State<'_, CaptureState>) -> Result<AnnotationSnapshot, String> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "состояние захвата повреждено".to_string())?;
    match guard.as_ref() {
        None => Ok(AnnotationSnapshot::default()),
        Some(active) => {
            let ann = active
                .annotations
                .lock()
                .map_err(|_| "состояние разметки повреждено".to_string())?;
            Ok(ann.snapshot())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::annotations::verify_annotation_chain;

    #[test]
    fn record_chains_and_folds() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = Arc::new(Mutex::new(Journal::open(tmp.path()).unwrap()));
        let mut st = AnnotationState::new(journal);

        st.record(
            AnnotationAction::MarkerAdded,
            "m1".into(),
            Some("Инцидент".into()),
            None,
            None,
            100,
            2,
            "op".into(),
            1_700_000_000_100,
        )
        .unwrap();
        st.record(
            AnnotationAction::RoleStarted,
            "r1".into(),
            None,
            Some("judge".into()),
            None,
            200,
            4,
            "op".into(),
            1_700_000_000_200,
        )
        .unwrap();

        // Цепочка целостна, свёртка даёт метку + открытый интервал.
        assert!(verify_annotation_chain(&st.entries));
        let snap = st.snapshot();
        assert_eq!(snap.markers.len(), 1);
        assert_eq!(snap.role_spans.len(), 1);
        assert_eq!(snap.role_spans[0].end_offset_samples, None);

        // Запись попала в журнал (переживёт рестарт).
        let state = crate::recorder::journal::replay(
            &tmp.path().join(crate::recorder::journal::JOURNAL_FILE_NAME),
        )
        .unwrap();
        assert_eq!(state.annotations.len(), 2);
    }

    #[test]
    fn end_open_role_spans_closes_previous_speaker() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = Arc::new(Mutex::new(Journal::open(tmp.path()).unwrap()));
        let mut st = AnnotationState::new(journal);

        // Первый говорящий — интервал открыт.
        st.record(
            AnnotationAction::RoleStarted,
            "r1".into(),
            None,
            Some("judge".into()),
            None,
            100,
            2,
            "op".into(),
            1_700_000_000_100,
        )
        .unwrap();
        assert_eq!(st.snapshot().role_spans.iter().filter(|s| s.end_offset_ms.is_none()).count(), 1);

        // Отмечаем следующего: авто-завершение закрывает предыдущего в тот же момент.
        let closed = st
            .end_open_role_spans(200, 4, "op", 1_700_000_000_200)
            .unwrap();
        assert_eq!(closed, vec!["r1".to_string()]);
        st.record(
            AnnotationAction::RoleStarted,
            "r2".into(),
            None,
            Some("defense".into()),
            None,
            200,
            4,
            "op".into(),
            1_700_000_000_200,
        )
        .unwrap();

        let snap = st.snapshot();
        assert_eq!(snap.role_spans.len(), 2);
        // r1 закрыт на 4 мс, r2 открыт.
        let r1 = snap.role_spans.iter().find(|s| s.id == "r1").unwrap();
        let r2 = snap.role_spans.iter().find(|s| s.id == "r2").unwrap();
        assert_eq!(r1.end_offset_ms, Some(4));
        assert_eq!(r2.end_offset_ms, None);
        // Цепочка целостна после авто-завершения.
        assert!(verify_annotation_chain(&st.entries));
    }

    #[test]
    fn end_open_role_spans_noop_when_none_open() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = Arc::new(Mutex::new(Journal::open(tmp.path()).unwrap()));
        let mut st = AnnotationState::new(journal);
        let closed = st.end_open_role_spans(0, 0, "op", 1).unwrap();
        assert!(closed.is_empty());
        assert!(st.entries.is_empty()); // ничего не записали
    }
}
