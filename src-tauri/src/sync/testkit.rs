//! Фейк-транспорт для оффлайн-тестов агента выгрузки (только `#[cfg(test)]`).
//!
//! Реализует [`UploadTransport`] в памяти с инъекцией сбоев: временные/постоянные
//! отказы частей, отказы регистрации/`init`/`complete` заданное число раз,
//! настраиваемый исход `verify`. Учитывает принятые части в множестве —
//! повторный `upload_part` идемпотентен (часть не дублируется). Доступен
//! юнит-тестам модулей `sync/*`; интеграционный тест держит свой фейк.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use super::client::{SessionMeta, TransportError, UploadTransport, VerifyOutcome};

/// Настройка инъекции сбоев фейка.
#[derive(Debug, Clone, Default)]
pub struct FakeConfig {
    /// Исход серверной верификации.
    pub verify_result: bool,
    /// Эти части один раз падают с временной ошибкой (потом проходят) — модель
    /// обрыва сети посреди выгрузки.
    pub fail_parts_transient_once: Vec<u32>,
    /// Эти части всегда падают постоянной ошибкой (4xx-аналог).
    pub fail_parts_permanent: Vec<u32>,
    /// Сколько раз `register_session` падает временной ошибкой перед успехом.
    pub fail_register_transient_times: u32,
    /// Сколько раз `init_upload` падает временной ошибкой перед успехом.
    pub fail_init_transient_times: u32,
    /// Сколько раз `complete_upload` падает временной ошибкой перед успехом.
    pub fail_complete_transient_times: u32,
}

#[derive(Default)]
struct FakeState {
    registrations: u32,
    init_calls: u32,
    complete_calls: u32,
    /// Принятые части по `(recording_id, part_index)` — проверка идемпотентности.
    received: HashSet<(String, u32, u32)>,
    /// Байты последней принятой версии части (контроль контента).
    bytes: HashMap<(String, u32, u32), Vec<u8>>,
    /// Уже «израсходованные» одноразовые сбои частей.
    transient_once_used: HashSet<u32>,
    register_fails_left: u32,
    init_fails_left: u32,
    complete_fails_left: u32,
}

/// Фейк-транспорт выгрузки.
pub struct FakeTransport {
    recording_id: String,
    cfg: FakeConfig,
    state: Mutex<FakeState>,
}

impl FakeTransport {
    /// Счастливый путь: всё проходит, `verify = true`.
    pub fn happy() -> Self {
        Self::new(FakeConfig {
            verify_result: true,
            ..Default::default()
        })
    }

    /// Фейк с заданной конфигурацией сбоев.
    pub fn new(cfg: FakeConfig) -> Self {
        let state = FakeState {
            register_fails_left: cfg.fail_register_transient_times,
            init_fails_left: cfg.fail_init_transient_times,
            complete_fails_left: cfg.fail_complete_transient_times,
            ..Default::default()
        };
        Self {
            recording_id: "rec-1".to_string(),
            cfg,
            state: Mutex::new(state),
        }
    }

    /// Сколько частей принято (для проверки докачки/идемпотентности).
    pub fn received_count(&self) -> usize {
        self.state.lock().unwrap().received.len()
    }

    /// Принята ли конкретная часть (по индексу, на любой дорожке).
    pub fn has_part(&self, part_index: u32) -> bool {
        self.state
            .lock()
            .unwrap()
            .received
            .iter()
            .any(|(rid, _tid, idx)| rid == &self.recording_id && *idx == part_index)
    }

    /// Принята ли часть конкретной дорожки.
    pub fn has_track_part(&self, track_id: u32, part_index: u32) -> bool {
        self.state
            .lock()
            .unwrap()
            .received
            .contains(&(self.recording_id.clone(), track_id, part_index))
    }

    /// Число вызовов `complete_upload` (контроль финализации).
    pub fn complete_calls(&self) -> u32 {
        self.state.lock().unwrap().complete_calls
    }
}

impl UploadTransport for FakeTransport {
    fn register_session(
        &self,
        _token: &str,
        _meta: &SessionMeta,
    ) -> Result<String, TransportError> {
        let mut st = self.state.lock().unwrap();
        if st.register_fails_left > 0 {
            st.register_fails_left -= 1;
            return Err(TransportError::transient("регистрация: нет сети"));
        }
        st.registrations += 1;
        Ok(self.recording_id.clone())
    }

    fn init_upload(
        &self,
        _token: &str,
        _recording_id: &str,
        _tracks: &[crate::store::export::TrackEntry],
        _annotations: &crate::store::export::AnnotationsExport,
    ) -> Result<(), TransportError> {
        let mut st = self.state.lock().unwrap();
        if st.init_fails_left > 0 {
            st.init_fails_left -= 1;
            return Err(TransportError::transient("init: нет сети"));
        }
        st.init_calls += 1;
        Ok(())
    }

    fn upload_part(
        &self,
        _token: &str,
        recording_id: &str,
        track_id: u32,
        part_index: u32,
        bytes: &[u8],
    ) -> Result<(), TransportError> {
        if self.cfg.fail_parts_permanent.contains(&part_index) {
            return Err(TransportError::permanent(format!(
                "часть {part_index} отклонена (4xx)"
            )));
        }
        let mut st = self.state.lock().unwrap();
        if self.cfg.fail_parts_transient_once.contains(&part_index)
            && !st.transient_once_used.contains(&part_index)
        {
            st.transient_once_used.insert(part_index);
            return Err(TransportError::transient(format!(
                "часть {part_index}: обрыв сети"
            )));
        }
        // Идемпотентность: повторный приём той же части не дублирует.
        st.received
            .insert((recording_id.to_string(), track_id, part_index));
        st.bytes
            .insert((recording_id.to_string(), track_id, part_index), bytes.to_vec());
        Ok(())
    }

    fn complete_upload(&self, _token: &str, _recording_id: &str) -> Result<(), TransportError> {
        let mut st = self.state.lock().unwrap();
        if st.complete_fails_left > 0 {
            st.complete_fails_left -= 1;
            return Err(TransportError::transient("complete: нет сети"));
        }
        st.complete_calls += 1;
        Ok(())
    }

    fn verify(&self, _token: &str, _recording_id: &str) -> Result<VerifyOutcome, TransportError> {
        Ok(VerifyOutcome {
            integrity_verified: self.cfg.verify_result,
        })
    }
}
