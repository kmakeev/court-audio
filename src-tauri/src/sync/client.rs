//! HTTP-транспорт выгрузки (этап 06 — `promts/06_sync_agent.md`, шаг 1).
//!
//! [`UploadTransport`] — seam контракта `07` (как `CaseDocketFetcher` этапа 05):
//! `register_session` → `init_upload` → `upload_part`×N → `complete_upload` →
//! `verify`. Реальная реализация [`HttpTransport`] на `reqwest::blocking` (JWT в
//! заголовке `Authorization: Bearer`); вся логика выгрузки тестируется оффлайн
//! на фейк-транспорте, поэтому здесь — только проводка и классификация ошибок.
//!
//! Классификация (для бэкоффа [`super::backoff_delay`]): `4xx` → [`ErrorKind::Permanent`]
//! (не ретраить), `5xx`/обрыв/таймаут → [`ErrorKind::Transient`] (ретраить).

use serde::{Deserialize, Serialize};

use crate::store::export::{AnnotationsExport, TrackEntry};

/// Класс ошибки транспорта: временная (ретраить) или постоянная (не ретраить).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Transient,
    Permanent,
}

/// Ошибка транспорта с классом и читаемым сообщением.
#[derive(Debug, Clone)]
pub struct TransportError {
    pub kind: ErrorKind,
    pub msg: String,
}

impl TransportError {
    pub fn transient(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Transient,
            msg: msg.into(),
        }
    }
    pub fn permanent(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Permanent,
            msg: msg.into(),
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.msg)
    }
}

/// Метаданные сессии для регистрации (`POST /audio/sessions/`). Идемпотентность —
/// по station-side `session_id` (как `get_or_create` у `UploadResult`, контракт `07`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub station_id: String,
    pub operator_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adjudication_ref: Option<String>,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub bit_depth: u16,
    /// Число дорожек записи (многоканал по ролям — этап 09; для v1 = 1).
    #[serde(default = "one_track")]
    pub track_count: u32,
}

fn one_track() -> u32 {
    1
}

/// Результат серверной верификации целостности (`POST /audio/recordings/<id>/verify/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyOutcome {
    pub integrity_verified: bool,
}

/// Ответ регистрации сессии. Сервер (`07`) отдаёт `recording_id` целым (PK
/// `AudioRecording`); в пайплайне выгрузки он используется как строковый
/// сегмент URL (`audio/recordings/<id>/...`), поэтому сразу приводим к строке.
#[derive(Debug, Clone, Deserialize)]
struct RegisterResponse {
    recording_id: i64,
}

/// Транспорт выгрузки по контракту `07`. `Send + Sync` — части грузятся
/// параллельно (до `sync.parallel_uploads`) через `std::thread::scope`.
pub trait UploadTransport: Send + Sync {
    /// Зарегистрировать сессию записи → серверный `recording_id` (идемпотентно
    /// по `meta.session_id`).
    fn register_session(&self, token: &str, meta: &SessionMeta) -> Result<String, TransportError>;

    /// Заявить состав записи: **дорожки** с ролями и их сегментами (размеры +
    /// SHA-256 + звенья цепочки) плюс **живую разметку** (метки/интервалы ролей —
    /// подсказки W2.11, этап 10). Роли доходят до диаризации W2.11 (этап 09).
    fn init_upload(
        &self,
        token: &str,
        recording_id: &str,
        tracks: &[TrackEntry],
        annotations: &AnnotationsExport,
    ) -> Result<(), TransportError>;

    /// Передать часть `(track_id, part_index)` (идемпотентно: повтор безопасен —
    /// докачка). Часть адресуется по дорожке (многоканал — этап 09).
    fn upload_part(
        &self,
        token: &str,
        recording_id: &str,
        track_id: u32,
        part_index: u32,
        bytes: &[u8],
    ) -> Result<(), TransportError>;

    /// Финализировать приём (сборка + сверка состава сегментов).
    fn complete_upload(&self, token: &str, recording_id: &str) -> Result<(), TransportError>;

    /// Запросить пересчёт целостности на сервере.
    fn verify(&self, token: &str, recording_id: &str) -> Result<VerifyOutcome, TransportError>;
}

/// Боевой транспорт на `reqwest::blocking`. Базовый URL — `sync.server_base_url`.
pub struct HttpTransport {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl HttpTransport {
    /// Создать клиент для базового URL `ex_system` (`sync.server_base_url`).
    pub fn new(base_url: impl Into<String>) -> Result<Self, TransportError> {
        let client = reqwest::blocking::Client::builder()
            .build()
            .map_err(|e| TransportError::transient(format!("инициализация HTTP-клиента: {e}")))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }
}

/// Классифицировать ошибку отправки запроса (обрыв/таймаут → временная).
fn classify_send(e: reqwest::Error) -> TransportError {
    TransportError::transient(format!("сетевой сбой: {e}"))
}

/// Проверить HTTP-статус: 2xx → Ok; 4xx → постоянная; иначе (5xx/прочее) → временная.
fn check_status(
    resp: reqwest::blocking::Response,
) -> Result<reqwest::blocking::Response, TransportError> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else if status.is_client_error() {
        Err(TransportError::permanent(format!(
            "сервер отклонил запрос ({status})"
        )))
    } else {
        Err(TransportError::transient(format!(
            "временная ошибка сервера ({status})"
        )))
    }
}

impl UploadTransport for HttpTransport {
    fn register_session(&self, token: &str, meta: &SessionMeta) -> Result<String, TransportError> {
        let resp = self
            .client
            .post(self.url("audio/sessions/"))
            .bearer_auth(token)
            .json(meta)
            .send()
            .map_err(classify_send)?;
        let resp = check_status(resp)?;
        let parsed: RegisterResponse = resp
            .json()
            .map_err(|e| TransportError::permanent(format!("разбор ответа регистрации: {e}")))?;
        Ok(parsed.recording_id.to_string())
    }

    fn init_upload(
        &self,
        token: &str,
        recording_id: &str,
        tracks: &[TrackEntry],
        annotations: &AnnotationsExport,
    ) -> Result<(), TransportError> {
        let resp = self
            .client
            .post(self.url(&format!("audio/recordings/{recording_id}/upload/init/")))
            .bearer_auth(token)
            .json(&serde_json::json!({ "tracks": tracks, "annotations": annotations }))
            .send()
            .map_err(classify_send)?;
        check_status(resp).map(|_| ())
    }

    fn upload_part(
        &self,
        token: &str,
        recording_id: &str,
        track_id: u32,
        part_index: u32,
        bytes: &[u8],
    ) -> Result<(), TransportError> {
        let resp = self
            .client
            .put(self.url(&format!(
                "audio/recordings/{recording_id}/upload/part/{track_id}/{part_index}/"
            )))
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes.to_vec())
            .send()
            .map_err(classify_send)?;
        check_status(resp).map(|_| ())
    }

    fn complete_upload(&self, token: &str, recording_id: &str) -> Result<(), TransportError> {
        let resp = self
            .client
            .post(self.url(&format!("audio/recordings/{recording_id}/upload/complete/")))
            .bearer_auth(token)
            .send()
            .map_err(classify_send)?;
        check_status(resp).map(|_| ())
    }

    fn verify(&self, token: &str, recording_id: &str) -> Result<VerifyOutcome, TransportError> {
        let resp = self
            .client
            .post(self.url(&format!("audio/recordings/{recording_id}/verify/")))
            .bearer_auth(token)
            .send()
            .map_err(classify_send)?;
        let resp = check_status(resp)?;
        resp.json()
            .map_err(|e| TransportError::permanent(format!("разбор ответа verify: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_join_is_clean() {
        let t = HttpTransport::new("https://ex.example/").unwrap();
        assert_eq!(
            t.url("/audio/sessions/"),
            "https://ex.example/audio/sessions/"
        );
        assert_eq!(
            t.url("audio/sessions/"),
            "https://ex.example/audio/sessions/"
        );
    }

    #[test]
    fn transport_error_constructors() {
        assert_eq!(TransportError::transient("x").kind, ErrorKind::Transient);
        assert_eq!(TransportError::permanent("x").kind, ErrorKind::Permanent);
    }
}
