//! Нормализация формата перед записью (этап 01 — `promts/01_audio_core.md`,
//! deliverable 3). **Без ресемпла частоты** — пишем на нативной частоте
//! устройства (согласовано с заказчиком). Здесь только дешёвые операции:
//!
//! 1. [`downmix`] — свести `native_channels` интерливнутых каналов к целевому
//!    числу каналов (в v1 — моно, усреднением).
//! 2. [`quantize_i16`] — привести `f32`-семплы (диапазон `[-1.0, 1.0]`) к PCM
//!    `i16` с клиппингом.
//!
//! Все размеры/частоты — снаружи (из `Settings.audio`); магических чисел нет.

/// Свести интерливнутые кадры из `native_channels` каналов к `target_channels`.
///
/// В v1 поддержан только `target_channels == 1` (моно усреднением каналов) —
/// многоканал по ролям отнесён к фазе 2 (`promts/09_multichannel.md`). Для
/// `native_channels == target_channels` вход возвращается без изменений.
///
/// `input` — интерливнут: `[c0,c1,…,cN-1, c0,c1,…]`. Неполный «хвостовой» кадр
/// (длина не кратна `native_channels`) отбрасывается.
pub fn downmix(input: &[f32], native_channels: u16, target_channels: u16) -> Vec<f32> {
    debug_assert!(native_channels >= 1, "native_channels должно быть >= 1");
    debug_assert!(target_channels >= 1, "target_channels должно быть >= 1");

    if native_channels == target_channels {
        return input.to_vec();
    }

    // v1: единственное поддержанное приведение — сведение в моно.
    assert_eq!(
        target_channels, 1,
        "downmix в v1 поддерживает только target_channels == 1 (моно)"
    );

    let n = native_channels as usize;
    let frames = input.len() / n;
    let mut out = Vec::with_capacity(frames);
    let inv = 1.0 / n as f32;
    for frame in input.chunks_exact(n) {
        let sum: f32 = frame.iter().sum();
        out.push(sum * inv);
    }
    out
}

/// Квантование нормированных `f32`-семплов (`[-1.0, 1.0]`) в PCM `i16` с
/// клиппингом выходящих за диапазон значений. NaN трактуется как тишина (0).
pub fn quantize_i16(input: &[f32]) -> Vec<i16> {
    input.iter().map(|&s| sample_to_i16(s)).collect()
}

/// Преобразовать один нормированный семпл в `i16` с клиппингом по диапазону.
fn sample_to_i16(s: f32) -> i16 {
    if s.is_nan() {
        return 0;
    }
    let clamped = s.clamp(-1.0, 1.0);
    // Масштабируем по положительному пределу i16 и округляем к ближайшему;
    // clamp гарантирует попадание в [i16::MIN, i16::MAX].
    let scaled = (clamped * i16::MAX as f32).round();
    scaled as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_stereo_to_mono_averages() {
        // [L,R, L,R] -> среднее по кадру.
        let input = [1.0, 0.0, 0.5, -0.5];
        let mono = downmix(&input, 2, 1);
        assert_eq!(mono, vec![0.5, 0.0]);
    }

    #[test]
    fn downmix_same_channel_count_is_identity() {
        let input = [0.1, -0.2, 0.3];
        assert_eq!(downmix(&input, 1, 1), input.to_vec());
    }

    #[test]
    fn downmix_drops_incomplete_trailing_frame() {
        // 5 семплов при 2 каналах -> 2 полных кадра, последний неполный отброшен.
        let input = [1.0, 1.0, 0.0, 0.0, 0.5];
        assert_eq!(downmix(&input, 2, 1), vec![1.0, 0.0]);
    }

    #[test]
    fn downmix_empty_input_does_not_panic() {
        assert!(downmix(&[], 2, 1).is_empty());
        assert!(downmix(&[], 1, 1).is_empty());
    }

    #[test]
    fn quantize_maps_range_endpoints() {
        let q = quantize_i16(&[0.0, 1.0, -1.0]);
        assert_eq!(q[0], 0);
        assert_eq!(q[1], i16::MAX);
        assert_eq!(q[2], -i16::MAX); // симметрично; -32767, не -32768
    }

    #[test]
    fn quantize_clips_out_of_range() {
        let q = quantize_i16(&[2.0, -2.0]);
        assert_eq!(q[0], i16::MAX);
        assert_eq!(q[1], -i16::MAX);
    }

    #[test]
    fn quantize_nan_is_silence() {
        assert_eq!(quantize_i16(&[f32::NAN]), vec![0]);
    }

    #[test]
    fn quantize_empty_input_does_not_panic() {
        assert!(quantize_i16(&[]).is_empty());
    }
}
