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

/// Выбрать один канал `channel_index` (0-based) из интерливнутого потока
/// `native_channels` каналов — извлечение дорожки многоканала (этап 09,
/// `promts/09_multichannel.md`, шаг 2). Неполный «хвостовой» кадр отбрасывается.
///
/// Если `channel_index >= native_channels`, канал считается отсутствующим и
/// возвращается тишина той же длины по кадрам (устойчивость к рассогласованию
/// конфигурации и реального формата устройства).
pub fn select_channel(input: &[f32], native_channels: u16, channel_index: u16) -> Vec<f32> {
    debug_assert!(native_channels >= 1, "native_channels должно быть >= 1");
    let n = native_channels as usize;
    let ch = channel_index as usize;
    let frames = input.len() / n;
    let mut out = Vec::with_capacity(frames);
    if ch >= n {
        out.resize(frames, 0.0);
        return out;
    }
    for frame in input.chunks_exact(n) {
        out.push(frame[ch]);
    }
    out
}

/// Свести N синхронных моно-дорожек в один мастер усреднением (опц.
/// `audio.master_downmix`, этап 09). Дорожки могут различаться по длине (дрейф/
/// обрыв) — микс идёт по максимальной длине, отсутствующие семплы считаются
/// тишиной; делитель на каждом отсчёте — число дорожек, реально имеющих семпл.
pub fn mix_tracks(tracks: &[&[i16]]) -> Vec<i16> {
    if tracks.is_empty() {
        return Vec::new();
    }
    let len = tracks.iter().map(|t| t.len()).max().unwrap_or(0);
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let mut sum: i32 = 0;
        let mut present: i32 = 0;
        for t in tracks {
            if let Some(&s) = t.get(i) {
                sum += s as i32;
                present += 1;
            }
        }
        let mixed = if present > 0 { sum / present } else { 0 };
        out.push(mixed as i16);
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

    #[test]
    fn select_channel_picks_requested_lane() {
        // [L,R, L,R] -> канал 0 = [L,L], канал 1 = [R,R].
        let input = [1.0, 0.25, 0.5, -0.5];
        assert_eq!(select_channel(&input, 2, 0), vec![1.0, 0.5]);
        assert_eq!(select_channel(&input, 2, 1), vec![0.25, -0.5]);
    }

    #[test]
    fn select_channel_drops_incomplete_trailing_frame() {
        let input = [1.0, 2.0, 3.0, 4.0, 5.0]; // 4-канальный: 1 полный кадр + хвост
        assert_eq!(select_channel(&input, 4, 2), vec![3.0]);
    }

    #[test]
    fn select_channel_missing_channel_is_silence() {
        // Запрошен канал вне числа каналов устройства → тишина по кадрам.
        let input = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(select_channel(&input, 2, 5), vec![0.0, 0.0]);
    }

    #[test]
    fn mix_tracks_averages_equal_length() {
        let a: Vec<i16> = vec![100, -100, 0];
        let b: Vec<i16> = vec![300, 100, 40];
        let mixed = mix_tracks(&[&a, &b]);
        assert_eq!(mixed, vec![200, 0, 20]);
    }

    #[test]
    fn mix_tracks_handles_ragged_lengths() {
        // Вторая дорожка короче (обрыв/дрейф): недостающие семплы = тишина,
        // делитель — число реально присутствующих дорожек на отсчёте.
        let a: Vec<i16> = vec![100, 200, 300];
        let b: Vec<i16> = vec![100];
        let mixed = mix_tracks(&[&a, &b]);
        assert_eq!(mixed, vec![100, 200, 300]);
    }

    #[test]
    fn mix_tracks_empty_input_is_empty() {
        let empty: Vec<&[i16]> = Vec::new();
        assert!(mix_tracks(&empty).is_empty());
    }
}
