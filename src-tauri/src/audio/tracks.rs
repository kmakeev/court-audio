//! Карта «канал ↔ роль» и разрешение состава дорожек (этап 09 —
//! `promts/09_multichannel.md`, шаг 1 / deliverable 2).
//!
//! Многоканал **аддитивен**: при выключенном `audio.multichannel.enabled` или
//! пустом `audio.tracks` разрешается ровно одна дорожка (`track_id = 0`) из
//! одиночного `audio.device` — это поведение v1. Все параметры — из
//! [`crate::settings`] (реестр `docs/configuration.md`), без «магических чисел».

use crate::settings::{Settings, TrackConfig};

use super::AudioError;

/// Роль по умолчанию для единственной дорожки в одноканальном (v1) режиме.
/// Не «магическое число логики», а метка легаси-совместимости манифеста.
pub const SINGLE_TRACK_ROLE: &str = "single";

/// Разрешённая (готовая к захвату) дорожка: стабильный `track_id`, источник и
/// назначенная роль. `channel_index` — индекс канала в интерливнутом потоке
/// устройства (0-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTrack {
    pub track_id: u32,
    pub device: Option<String>,
    pub channel_index: u16,
    pub role: String,
    pub label: String,
}

/// Построить состав дорожек из настроек.
///
/// - Многоканал выключен **или** `audio.tracks` пуст → один трек `track_id = 0`
///   из `audio.device` (роль [`SINGLE_TRACK_ROLE`]) — поведение v1.
/// - Иначе — по одной [`ResolvedTrack`] на запись `audio.tracks`, с валидацией
///   роли по справочнику `audio.roles`.
pub fn resolve_tracks(settings: &Settings) -> Result<Vec<ResolvedTrack>, AudioError> {
    let audio = &settings.audio;

    if !audio.multichannel.enabled || audio.tracks.is_empty() {
        return Ok(vec![ResolvedTrack {
            track_id: 0,
            device: audio.device.clone(),
            channel_index: 0,
            role: SINGLE_TRACK_ROLE.to_string(),
            label: single_track_label(audio.device.as_deref()),
        }]);
    }

    if audio.roles.is_empty() {
        return Err(AudioError::TrackConfig(
            "справочник ролей audio.roles пуст".to_string(),
        ));
    }

    let master = audio.sync.clock_master_track as usize;
    if master >= audio.tracks.len() {
        return Err(AudioError::TrackConfig(format!(
            "clock_master_track={master} вне диапазона дорожек (0..{})",
            audio.tracks.len()
        )));
    }

    let mut resolved = Vec::with_capacity(audio.tracks.len());
    for (idx, tc) in audio.tracks.iter().enumerate() {
        validate_role(&tc.role, &audio.roles)?;
        resolved.push(ResolvedTrack {
            track_id: idx as u32,
            device: track_device(tc, audio.device.as_deref()),
            channel_index: tc.channel_index,
            role: tc.role.clone(),
            label: track_label(tc),
        });
    }
    Ok(resolved)
}

/// Устройство дорожки: явное из `TrackConfig`, иначе — общий `audio.device`.
fn track_device(tc: &TrackConfig, default_device: Option<&str>) -> Option<String> {
    tc.device
        .clone()
        .or_else(|| default_device.map(|s| s.to_string()))
}

/// Метка дорожки: явная, иначе — роль (для UI и имён каталогов дорожек).
fn track_label(tc: &TrackConfig) -> String {
    if tc.label.trim().is_empty() {
        tc.role.clone()
    } else {
        tc.label.clone()
    }
}

fn single_track_label(device: Option<&str>) -> String {
    device.map(|s| s.to_string()).unwrap_or_default()
}

fn validate_role(role: &str, roles: &[String]) -> Result<(), AudioError> {
    if role.trim().is_empty() {
        return Err(AudioError::TrackConfig("пустая роль дорожки".to_string()));
    }
    if !roles.iter().any(|r| r == role) {
        return Err(AudioError::TrackConfig(format!(
            "роль «{role}» вне справочника audio.roles"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(role: &str, channel: u16) -> TrackConfig {
        TrackConfig {
            device: None,
            channel_index: channel,
            role: role.to_string(),
            label: String::new(),
        }
    }

    #[test]
    fn disabled_multichannel_yields_single_track() {
        let s = Settings::default();
        let tracks = resolve_tracks(&s).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].track_id, 0);
        assert_eq!(tracks[0].role, SINGLE_TRACK_ROLE);
        assert_eq!(tracks[0].channel_index, 0);
    }

    #[test]
    fn empty_tracks_list_yields_single_track_even_if_enabled() {
        let mut s = Settings::default();
        s.audio.multichannel.enabled = true;
        s.audio.tracks.clear();
        let tracks = resolve_tracks(&s).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].role, SINGLE_TRACK_ROLE);
    }

    #[test]
    fn multichannel_resolves_roles_and_stable_ids() {
        let mut s = Settings::default();
        s.audio.multichannel.enabled = true;
        s.audio.tracks = vec![track("judge", 0), track("defense", 1)];
        let tracks = resolve_tracks(&s).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].track_id, 0);
        assert_eq!(tracks[0].role, "judge");
        assert_eq!(tracks[1].track_id, 1);
        assert_eq!(tracks[1].role, "defense");
        assert_eq!(tracks[1].channel_index, 1);
        // Метка по умолчанию = роль.
        assert_eq!(tracks[1].label, "defense");
    }

    #[test]
    fn role_outside_dictionary_is_rejected() {
        let mut s = Settings::default();
        s.audio.multichannel.enabled = true;
        s.audio.tracks = vec![track("judge", 0), track("bailiff", 1)];
        let err = resolve_tracks(&s).unwrap_err();
        assert!(matches!(err, AudioError::TrackConfig(_)));
    }

    #[test]
    fn clock_master_out_of_range_is_rejected() {
        let mut s = Settings::default();
        s.audio.multichannel.enabled = true;
        s.audio.tracks = vec![track("judge", 0)];
        s.audio.sync.clock_master_track = 3;
        assert!(matches!(
            resolve_tracks(&s),
            Err(AudioError::TrackConfig(_))
        ));
    }

    #[test]
    fn explicit_label_and_device_are_preserved() {
        let mut s = Settings::default();
        s.audio.multichannel.enabled = true;
        s.audio.tracks = vec![TrackConfig {
            device: Some("USB Mic 1".to_string()),
            channel_index: 0,
            role: "witness".to_string(),
            label: "Свидетель у трибуны".to_string(),
        }];
        let tracks = resolve_tracks(&s).unwrap();
        assert_eq!(tracks[0].device.as_deref(), Some("USB Mic 1"));
        assert_eq!(tracks[0].label, "Свидетель у трибуны");
    }
}
