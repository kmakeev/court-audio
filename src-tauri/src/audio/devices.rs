//! Перечисление устройств ввода и их возможностей (этап 01 —
//! `promts/01_audio_core.md`, шаг 2). Используется Tauri-командой
//! `list_audio_devices` для экрана настроек/диагностики (этап 04).

use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;

use super::AudioError;

/// Диапазон поддерживаемой конфигурации устройства (одна строка из
/// `cpal::SupportedStreamConfigRange`).
#[derive(Debug, Clone, Serialize)]
pub struct ConfigRange {
    pub channels: u16,
    pub min_sample_rate_hz: u32,
    pub max_sample_rate_hz: u32,
    /// Имя sample-формата (`f32`, `i16`, …) — для диагностики.
    pub sample_format: String,
}

/// Описание устройства ввода + его возможности.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub name: String,
    /// Является ли устройство системным по умолчанию.
    pub is_default: bool,
    /// Частота конфигурации устройства по умолчанию (если доступна).
    pub default_sample_rate_hz: Option<u32>,
    /// Число каналов конфигурации по умолчанию (если доступно).
    pub default_channels: Option<u16>,
    pub configs: Vec<ConfigRange>,
}

/// Перечислить устройства ввода и их возможности. Пустой список — не ошибка
/// (UI покажет «нет устройств»); ошибкой считаем сбой перечисления хоста.
pub fn list_input_devices() -> Result<Vec<DeviceInfo>, AudioError> {
    let host = cpal::default_host();

    let default_name = host.default_input_device().and_then(|d| d.name().ok());

    let devices = host
        .input_devices()
        .map_err(|e| AudioError::Enumerate(e.to_string()))?;

    let mut out = Vec::new();
    for device in devices {
        let name = device
            .name()
            .map_err(|e| AudioError::Enumerate(e.to_string()))?;

        let configs = match device.supported_input_configs() {
            Ok(ranges) => ranges
                .map(|r| ConfigRange {
                    channels: r.channels(),
                    min_sample_rate_hz: r.min_sample_rate().0,
                    max_sample_rate_hz: r.max_sample_rate().0,
                    sample_format: format!("{:?}", r.sample_format()),
                })
                .collect(),
            // Возможности конкретного устройства могут не читаться — это не
            // повод ронять весь список (полноценная диагностика — этап 02).
            Err(_) => Vec::new(),
        };

        let default_cfg = device.default_input_config().ok();

        out.push(DeviceInfo {
            is_default: default_name.as_deref() == Some(name.as_str()),
            default_sample_rate_hz: default_cfg.as_ref().map(|c| c.sample_rate().0),
            default_channels: default_cfg.as_ref().map(|c| c.channels()),
            configs,
            name,
        });
    }

    Ok(out)
}
