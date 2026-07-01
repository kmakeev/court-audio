//! Захват звука (этап 01 — `promts/01_audio_core.md`).
//!
//! Подмодули:
//! - [`ring`] — lock-free SPSC кольцевой буфер (callback → consumer);
//! - [`convert`] — нормализация формата (downmix + квантование; без ресемпла);
//! - [`devices`] — перечисление устройств ввода через `cpal`;
//! - [`capture`] — конвейер захвата `cpal` → ring → сегментный райтер.
//!
//! Формат записи (решение заказчика): пишем на **нативной частоте** устройства,
//! приводим только sample-формат к PCM `bit_depth` и каналы к `channels`. Ресемпл
//! частоты в v1 не делаем (см. deliverable 3 промта).

use std::fmt;

pub mod capture;
pub mod convert;
pub mod devices;
pub mod ring;
pub mod sync;
pub mod tracks;

/// Ошибки слоя захвата звука. Текстовые детали — для логов/IPC (строкой в UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    /// Нет устройства ввода (пустой список / нет системного по умолчанию).
    /// Заглушка для этапа 01; полноценная обработка — этап 02.
    NoInputDevice,
    /// Запрошенное устройство по имени не найдено среди устройств ввода.
    DeviceNotFound(String),
    /// Не удалось перечислить устройства/возможности.
    Enumerate(String),
    /// Не удалось подобрать поддерживаемую конфигурацию устройства.
    Config(String),
    /// Формат семплов устройства не поддержан конвейером.
    UnsupportedFormat(String),
    /// Не удалось построить/запустить входной поток `cpal`.
    Stream(String),
    /// Ошибка файловой подсистемы при записи сегментов.
    Io(String),
    /// Некорректная карта дорожек `audio.tracks` (роль вне справочника,
    /// пустой состав и т.п.) — многоканал по ролям (этап 09).
    TrackConfig(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoInputDevice => write!(f, "нет доступного устройства ввода"),
            AudioError::DeviceNotFound(name) => {
                write!(f, "устройство ввода «{name}» не найдено")
            }
            AudioError::Enumerate(e) => write!(f, "не удалось перечислить устройства: {e}"),
            AudioError::Config(e) => write!(f, "не удалось подобрать конфигурацию: {e}"),
            AudioError::UnsupportedFormat(e) => write!(f, "формат семплов не поддержан: {e}"),
            AudioError::Stream(e) => write!(f, "ошибка входного потока: {e}"),
            AudioError::Io(e) => write!(f, "ошибка ввода-вывода: {e}"),
            AudioError::TrackConfig(e) => write!(f, "некорректная карта дорожек: {e}"),
        }
    }
}

impl std::error::Error for AudioError {}
