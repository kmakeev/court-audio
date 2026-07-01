//! Единый клок сессии и контроль дрейфа между дорожками (этап 09 —
//! `promts/09_multichannel.md`, шаг 3 / deliverable 3).
//!
//! Все дорожки сессии разделяют один таймкод. Одна дорожка объявляется
//! **мастером клока** (`audio.sync.clock_master_track`); остальные сверяются с
//! ней по числу произведённых кадров. При одном многоканальном интерфейсе
//! (общий аппаратный клок) дрейф ≈ 0 и коррекция не срабатывает; при раздельных
//! устройствах (набор USB-микрофонов) накапливается расхождение, которое здесь
//! измеряется, фиксируется в журнале и (по настройке) компенсируется pad/drop
//! семплов.
//!
//! Модуль **чист** (без I/O) — вся логика тестируется юнит-тестами. Пороги и
//! флаги — из [`crate::settings`] (`audio.sync.*`), без «магических чисел».

use crate::settings::AudioSyncSettings;

/// Коррекция дорожки для восстановления семпл-выравнивания с мастером.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftCorrection {
    /// Коррекция не требуется (в пороге либо компенсация выключена).
    None,
    /// Дорожка отстаёт от мастера — дописать столько семплов тишины.
    Pad(u64),
    /// Дорожка опережает мастер — отбросить столько последних семплов.
    Drop(u64),
}

/// Результат измерения дрейфа дорожки относительно мастера клока.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftReport {
    /// Знаковый дрейф в миллисекундах: `> 0` — дорожка опережает мастер.
    pub drift_ms: f64,
    /// Дрейф превысил порог `audio.sync.drift_threshold_ms`.
    pub exceeded: bool,
    /// Рекомендованная коррекция (если включена компенсация и порог превышен).
    pub correction: DriftCorrection,
}

/// Оценщик дрейфа. Держит частоту дискретизации и параметры из реестра.
#[derive(Debug, Clone)]
pub struct DriftEstimator {
    sample_rate_hz: u32,
    threshold_ms: u32,
    compensate: bool,
}

impl DriftEstimator {
    /// Собрать из частоты дискретизации и `audio.sync.*`.
    pub fn new(sample_rate_hz: u32, sync: &AudioSyncSettings) -> Self {
        Self {
            sample_rate_hz,
            threshold_ms: sync.drift_threshold_ms,
            compensate: sync.drift_compensate,
        }
    }

    /// Оценить дрейф дорожки по числу произведённых кадров относительно мастера.
    /// `master_frames`/`track_frames` — суммарно произведённые кадры за общий
    /// интервал (частоты дорожек совпадают — ресемпла нет, решение v1).
    pub fn estimate(&self, master_frames: u64, track_frames: u64) -> DriftReport {
        let drift_frames = track_frames as i64 - master_frames as i64;
        let drift_ms = if self.sample_rate_hz == 0 {
            0.0
        } else {
            drift_frames as f64 / self.sample_rate_hz as f64 * 1000.0
        };
        let exceeded = drift_ms.abs() > self.threshold_ms as f64;

        let correction = if !self.compensate || !exceeded {
            DriftCorrection::None
        } else if drift_frames > 0 {
            DriftCorrection::Drop(drift_frames as u64)
        } else {
            DriftCorrection::Pad((-drift_frames) as u64)
        };

        DriftReport {
            drift_ms,
            exceeded,
            correction,
        }
    }
}

/// Применить коррекцию к буферу семплов дорожки, сохраняя семпл-выравнивание.
/// `Pad` дописывает тишину в хвост, `Drop` отбрасывает хвостовые семплы (не
/// больше длины буфера). Возвращает скорректированный буфер.
pub fn apply_correction(samples: &[i16], correction: DriftCorrection) -> Vec<i16> {
    match correction {
        DriftCorrection::None => samples.to_vec(),
        DriftCorrection::Pad(n) => {
            let mut out = Vec::with_capacity(samples.len() + n as usize);
            out.extend_from_slice(samples);
            out.resize(samples.len() + n as usize, 0);
            out
        }
        DriftCorrection::Drop(n) => {
            let keep = samples.len().saturating_sub(n as usize);
            samples[..keep].to_vec()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync(threshold_ms: u32, compensate: bool) -> AudioSyncSettings {
        AudioSyncSettings {
            clock_master_track: 0,
            drift_threshold_ms: threshold_ms,
            drift_compensate: compensate,
        }
    }

    #[test]
    fn no_drift_when_frame_counts_match() {
        let est = DriftEstimator::new(48_000, &sync(50, true));
        let r = est.estimate(48_000, 48_000);
        assert_eq!(r.drift_ms, 0.0);
        assert!(!r.exceeded);
        assert_eq!(r.correction, DriftCorrection::None);
    }

    #[test]
    fn small_drift_below_threshold_is_not_corrected() {
        // 48 кадров при 48 кГц = 1 мс < порог 50 мс.
        let est = DriftEstimator::new(48_000, &sync(50, true));
        let r = est.estimate(48_000, 48_048);
        assert!(r.drift_ms > 0.9 && r.drift_ms < 1.1);
        assert!(!r.exceeded);
        assert_eq!(r.correction, DriftCorrection::None);
    }

    #[test]
    fn track_ahead_over_threshold_drops_samples() {
        // +4800 кадров при 48 кГц = +100 мс > порог 50 мс, дорожка опережает.
        let est = DriftEstimator::new(48_000, &sync(50, true));
        let r = est.estimate(48_000, 52_800);
        assert!(r.exceeded);
        assert!(r.drift_ms > 0.0);
        assert_eq!(r.correction, DriftCorrection::Drop(4_800));
    }

    #[test]
    fn track_behind_over_threshold_pads_samples() {
        // -4800 кадров = -100 мс, дорожка отстаёт → добить тишиной.
        let est = DriftEstimator::new(48_000, &sync(50, true));
        let r = est.estimate(52_800, 48_000);
        assert!(r.exceeded);
        assert!(r.drift_ms < 0.0);
        assert_eq!(r.correction, DriftCorrection::Pad(4_800));
    }

    #[test]
    fn compensation_disabled_reports_but_does_not_correct() {
        let est = DriftEstimator::new(48_000, &sync(50, false));
        let r = est.estimate(48_000, 52_800);
        assert!(r.exceeded);
        assert_eq!(r.correction, DriftCorrection::None);
    }

    #[test]
    fn apply_pad_appends_silence_preserving_prefix() {
        let out = apply_correction(&[10, 20, 30], DriftCorrection::Pad(2));
        assert_eq!(out, vec![10, 20, 30, 0, 0]);
    }

    #[test]
    fn apply_drop_removes_trailing_samples() {
        let out = apply_correction(&[10, 20, 30, 40], DriftCorrection::Drop(2));
        assert_eq!(out, vec![10, 20]);
    }

    #[test]
    fn apply_drop_saturates_at_buffer_length() {
        let out = apply_correction(&[10, 20], DriftCorrection::Drop(5));
        assert!(out.is_empty());
    }

    #[test]
    fn apply_none_is_identity() {
        let out = apply_correction(&[1, 2, 3], DriftCorrection::None);
        assert_eq!(out, vec![1, 2, 3]);
    }
}
