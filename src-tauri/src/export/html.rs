//! Автономный HTML-плеер пакета (этап 10.2, шаг 2). Одна страница без
//! внешних зависимостей — данные внедряются `<script type="application/json">`
//! (не через `fetch()`: на `file://` это упирается в CORS в Chrome), аудио —
//! через один `<audio>`-элемент с относительным `src`, переключаемым при
//! нескольких файлах (медиаэлементы под ограничение `fetch()` не подпадают).
//! Раскладка/поведение (таймлайн, оглавление, хоткеи) повторяют встроенный
//! проигрыватель `Playback.tsx` (этап 10.1) — см. `promts/10_2_export.md`.

use serde::Serialize;

use super::ExportError;
use crate::integrity::annotations::{MarkerState, RoleSpanState};

const TEMPLATE: &str = include_str!("player_template.html");
const DATA_PLACEHOLDER: &str = "__EXPORT_PLAYER_DATA__";

/// Одна дорожка в данных плеера: относительный путь к файлу + подписи.
#[derive(Debug, Clone, Serialize)]
pub struct PlayerTrackView {
    /// Относительный путь к аудиофайлу внутри пакета (напр. `audio/judge.wav`).
    pub file: String,
    pub label: String,
    pub role: String,
}

/// Данные, встраиваемые в HTML-плеер. `seek_step_seconds`/`playback_rates` —
/// снимок `Settings.player.*` на момент экспорта (страница офлайн, IPC к
/// `Settings` недоступен получателю копии).
#[derive(Debug, Clone, Serialize)]
pub struct PlayerData {
    pub session_id: String,
    pub started_at_unix_ms: u64,
    pub adjudication_ref: Option<String>,
    pub tracks: Vec<PlayerTrackView>,
    pub markers: Vec<MarkerState>,
    pub role_spans: Vec<RoleSpanState>,
    pub duration_ms: u64,
    pub seek_step_seconds: f32,
    pub playback_rates: Vec<f32>,
}

/// Отрисовать HTML-плеер: подставить JSON `data` вместо плейсхолдера в
/// статичном шаблоне (простая `.replace()`, без внешней шаблонизации).
/// `</` в данных экранируется как `<\/`, чтобы HTML-парсер не закрыл
/// `<script>` раньше времени, встреть он `</script` внутри комментария метки.
pub fn render(data: &PlayerData) -> Result<String, ExportError> {
    let json = serde_json::to_string(data)?;
    let escaped = json.replace("</", "<\\/");
    Ok(TEMPLATE.replacen(DATA_PLACEHOLDER, &escaped, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_data() -> PlayerData {
        PlayerData {
            session_id: "sess-1".to_string(),
            started_at_unix_ms: 1_700_000_000_000,
            adjudication_ref: Some("№ 1-123/2026".to_string()),
            tracks: vec![
                PlayerTrackView {
                    file: "audio/judge.wav".to_string(),
                    label: "Судья".to_string(),
                    role: "judge".to_string(),
                },
                PlayerTrackView {
                    file: "audio/defense.wav".to_string(),
                    label: "Защита".to_string(),
                    role: "defense".to_string(),
                },
            ],
            markers: vec![MarkerState {
                id: "m1".to_string(),
                category: "Инцидент".to_string(),
                comment: Some("шум в зале".to_string()),
                offset_samples: 44_100,
                offset_ms: 1_000,
                operator_id: "op-1".to_string(),
                at_unix_ms: 1_700_000_001_000,
            }],
            role_spans: vec![RoleSpanState {
                id: "r1".to_string(),
                role: "judge".to_string(),
                start_offset_samples: 88_200,
                start_offset_ms: 2_000,
                end_offset_samples: Some(132_300),
                end_offset_ms: Some(3_000),
                operator_id: "op-1".to_string(),
                at_unix_ms: 1_700_000_002_000,
            }],
            duration_ms: 125_000,
            seek_step_seconds: 15.0,
            playback_rates: vec![0.5, 1.0, 2.0],
        }
    }

    #[test]
    fn render_embeds_data_as_valid_json_script() {
        let html = render(&sample_data()).unwrap();
        assert!(html.contains(r#"<script id="export-data" type="application/json">"#));
        let start = html.find(r#"type="application/json">"#).unwrap()
            + r#"type="application/json">"#.len();
        let end = html[start..].find("</script>").unwrap() + start;
        let raw = &html[start..end];
        let parsed: serde_json::Value = serde_json::from_str(raw).expect("встроенный блок — валидный JSON");
        assert_eq!(parsed["session_id"], "sess-1");
        assert_eq!(parsed["tracks"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn render_escapes_script_close_tag_in_marker_comment() {
        let mut data = sample_data();
        data.markers[0].comment = Some("</script><script>alert(1)</script>".to_string());
        let html = render(&data).unwrap();
        // Ровно два настоящих закрывающих тега — блок данных и блок логики
        // шаблона; экранирование не добавляет (и не теряет) ни одного.
        assert_eq!(html.matches("</script>").count(), 2);
        assert!(!html.contains("</script><script>alert"));

        let start = html.find(r#"type="application/json">"#).unwrap()
            + r#"type="application/json">"#.len();
        let end = html[start..].find("</script>").unwrap() + start;
        let raw = &html[start..end];
        let parsed: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed["markers"][0]["comment"],
            "</script><script>alert(1)</script>"
        );
    }

    #[test]
    fn render_contains_no_external_resource_references() {
        let html = render(&sample_data()).unwrap();
        assert!(!html.contains("http://"));
        assert!(!html.contains("https://"));
        assert!(!html.contains("<script src="));
        assert!(!html.contains("<link href="));
    }

    #[test]
    fn render_contains_exactly_one_audio_element() {
        let html = render(&sample_data()).unwrap();
        assert_eq!(html.matches("<audio").count(), 1);
    }

    #[test]
    fn render_includes_seek_step_and_playback_rates() {
        let html = render(&sample_data()).unwrap();
        let start = html.find(r#"type="application/json">"#).unwrap()
            + r#"type="application/json">"#.len();
        let end = html[start..].find("</script>").unwrap() + start;
        let parsed: serde_json::Value = serde_json::from_str(&html[start..end]).unwrap();
        assert_eq!(parsed["seek_step_seconds"], 15.0);
        assert_eq!(parsed["playback_rates"].as_array().unwrap().len(), 3);
    }
}
