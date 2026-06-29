//! Контроль свободного места — этап 02 (`promts/02_recorder_reliability.md`,
//! deliverable 6).
//!
//! Сравнивает свободное место на томе хранилища с порогами
//! `reliability.disk_low_threshold_mb` (предупреждение) и
//! `reliability.disk_critical_mb` (защитные действия). По решению заказчика при
//! критическом пороге — **корректный стоп с гарантированным флашем** (без потери
//! записанного), а не переключение на зеркало.
//!
//! Классификация — чистая функция [`classify`] (тестируется без диска). Реальный
//! замер свободного места — [`free_space_mb`] через кроссплатформенный `fs2`
//! (без ОС-специфики: ALSA/WASAPI/CoreAudio-нейтрально).

use std::path::Path;

/// Состояние свободного места.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskStatus {
    /// Места достаточно.
    Ok,
    /// Ниже порога предупреждения.
    Low,
    /// Достигнут критический порог (нужны защитные действия).
    Critical,
}

/// Пороги свободного места (из `Settings.reliability`), в мегабайтах.
#[derive(Debug, Clone, Copy)]
pub struct DiskThresholds {
    pub low_mb: u64,
    pub critical_mb: u64,
}

/// Классифицировать свободное место. `Critical` имеет приоритет над `Low`
/// (критический порог ниже порога предупреждения).
pub fn classify(free_mb: u64, thresholds: DiskThresholds) -> DiskStatus {
    if free_mb <= thresholds.critical_mb {
        DiskStatus::Critical
    } else if free_mb <= thresholds.low_mb {
        DiskStatus::Low
    } else {
        DiskStatus::Ok
    }
}

/// Свободное место (МиБ) на томе, содержащем `path`. Кроссплатформенно через
/// `fs2::available_space` (учитывает доступное непривилегированному процессу).
pub fn free_space_mb(path: &Path) -> std::io::Result<u64> {
    // available_space принимает существующий путь; для каталога сессии это его
    // родитель/корень хранилища (создаётся до старта записи).
    let bytes = fs2::available_space(path)?;
    Ok(bytes / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TH: DiskThresholds = DiskThresholds {
        low_mb: 1_024,
        critical_mb: 256,
    };

    #[test]
    fn classify_ok_above_low() {
        assert_eq!(classify(2_048, TH), DiskStatus::Ok);
        assert_eq!(classify(1_025, TH), DiskStatus::Ok);
    }

    #[test]
    fn classify_low_between_thresholds() {
        assert_eq!(classify(1_024, TH), DiskStatus::Low); // ровно порог = Low
        assert_eq!(classify(512, TH), DiskStatus::Low);
        assert_eq!(classify(257, TH), DiskStatus::Low);
    }

    #[test]
    fn classify_critical_at_or_below() {
        assert_eq!(classify(256, TH), DiskStatus::Critical); // ровно порог = Critical
        assert_eq!(classify(0, TH), DiskStatus::Critical);
    }

    #[test]
    fn free_space_of_existing_dir_is_positive() {
        let tmp = tempfile::tempdir().unwrap();
        let free = free_space_mb(tmp.path()).unwrap();
        // На рабочей машине/CI на временном томе всегда есть место.
        assert!(free > 0);
    }
}
