//! HTTP-фетчер докета дел (этап 05 deliverable 1 — подключение к серверному
//! slim-эндпоинту `07`). Реализует seam [`CaseDocketFetcher`] из стора поверх
//! `reqwest::blocking`: тянет `GET /audio/docket/` под станционным JWT и мапит
//! ответ в [`CaseRecord`].
//!
//! Скоуп докета (суд/зал) сервер определяет по идентичности станции
//! (`AudioStationProfile`), поэтому в запрос он не передаётся; клиентский
//! `scope` — лишь подпись кэша. Пагинацию обходим запросом `page_size=limit`
//! (сервер ограничивает `max_records`/`max_page_size`).

use serde::Deserialize;

use crate::store::case_cache::{CaseDocketFetcher, CaseRecord};

/// Элемент ответа докета (`/audio/docket/`), slim-поля привязки.
#[derive(Debug, Deserialize)]
struct DocketItem {
    id: i64,
    #[serde(default)]
    case_number: String,
    #[serde(default)]
    case_receipt_date: Option<String>,
    #[serde(default)]
    defendant_fios: Vec<String>,
}

/// Пагинированный ответ (`StandardResultsSetPagination`): нужен только `results`.
#[derive(Debug, Deserialize)]
struct DocketResponse {
    results: Vec<DocketItem>,
}

/// Распарсить тело ответа докета в записи кэша (чистая, тестируемая без сети).
fn parse_docket_response(body: &str) -> Result<Vec<CaseRecord>, String> {
    let parsed: DocketResponse =
        serde_json::from_str(body).map_err(|e| format!("разбор докета: {e}"))?;
    Ok(parsed
        .results
        .into_iter()
        .map(|it| CaseRecord {
            id: it.id.to_string(),
            number: it.case_number,
            // ФИО подсудимых — одной строкой (как ожидает CaseRecord/поиск кэша).
            fio: it.defendant_fios.join(", "),
            date: it.case_receipt_date.unwrap_or_default(),
        })
        .collect())
}

/// HTTP-фетчер докета `ex_system` под станционным JWT.
pub struct DocketHttpFetcher {
    base_url: String,
    token: String,
    client: reqwest::blocking::Client,
}

impl DocketHttpFetcher {
    /// Создать фетчер для базового URL `ex_system` (`sync.server_base_url`) и
    /// операторского токена (`COURT_AUDIO_OPERATOR_TOKEN`).
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .build()
            .map_err(|e| format!("инициализация HTTP-клиента: {e}"))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
            client,
        })
    }
}

impl CaseDocketFetcher for DocketHttpFetcher {
    fn fetch(&self, _scope: &str, limit: u32) -> Result<Vec<CaseRecord>, String> {
        // page_size=limit — забираем докет одной страницей (сервер ограничивает
        // объём своим max_page_size; sync_into_cache дополнительно режет до max).
        let url = format!("{}/audio/docket/?page_size={}", self.base_url, limit);
        let resp = self
            .client
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| format!("сеть: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("сервер вернул {status}"));
        }
        let body = resp
            .text()
            .map_err(|e| format!("чтение ответа докета: {e}"))?;
        parse_docket_response(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docket_items_into_case_records() {
        let body = r#"{
            "count": 2,
            "results": [
                {"id": 444356, "case_number": "1-9/2026",
                 "case_receipt_date": "2026-05-01",
                 "defendant_fios": ["Иванов И. И.", "Петров П. П."]},
                {"id": 7, "case_number": "", "case_receipt_date": null,
                 "defendant_fios": []}
            ]
        }"#;
        let recs = parse_docket_response(body).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].id, "444356");
        assert_eq!(recs[0].number, "1-9/2026");
        assert_eq!(recs[0].fio, "Иванов И. И., Петров П. П.");
        assert_eq!(recs[0].date, "2026-05-01");
        // Пустые/null поля деградируют мягко.
        assert_eq!(recs[1].id, "7");
        assert_eq!(recs[1].number, "");
        assert_eq!(recs[1].fio, "");
        assert_eq!(recs[1].date, "");
    }

    #[test]
    fn rejects_malformed_body() {
        assert!(parse_docket_response("not json").is_err());
    }
}
