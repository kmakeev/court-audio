//! Потоковый декодер сегментов + `rodio::Source`-адаптеры (этап 10.1, шаги
//! 1–2). Декодирует **по одному сегменту** за раз (не всю сессию сразу) —
//! ограниченный объём в памяти (сегмент ≤ `recorder.segment_seconds`,
//! дефолт 30 с). Дешифровка — [`crate::store::crypto::read_segment_plain`],
//! ключ и plaintext не покидают ядро (не пишутся на диск, не идут через IPC).
//!
//! Ошибка декодирования сегмента **в середине потока** (повреждён/усечён —
//! обрыв питания, этап 02) не может быть проброшена наверх через `Iterator`
//! (у `rodio::Source` нет `Result`-варианта `next()`): поток просто
//! останавливается на этом месте, как если бы сессия на нём закончилась —
//! это и есть «не паникует» из критериев приёмки. Ошибка на **первом**
//! сегменте от точки сика ловится раньше, в [`seek_source`] (возвращает
//! `Result`), — так открытие/сик на полностью нечитаемую сессию сразу видно
//! вызывающему.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::Source;

use crate::store::crypto;

use super::timeline::{Timeline, TimelineSegment};

/// Ошибка ядра плеера.
#[derive(Debug)]
pub enum PlayerError {
    /// Не удалось разобрать WAV/дешифровать сегмент.
    Decode(String),
    /// Ошибка ключа/шифрования at-rest.
    Crypto(String),
}

impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlayerError::Decode(e) => write!(f, "ошибка декодирования сегмента: {e}"),
            PlayerError::Crypto(e) => write!(f, "ошибка дешифровки сегмента: {e}"),
        }
    }
}

impl std::error::Error for PlayerError {}

impl From<crypto::CryptoError> for PlayerError {
    fn from(e: crypto::CryptoError) -> Self {
        PlayerError::Crypto(e.to_string())
    }
}

/// Дешифровать (если нужно) и разобрать один сегмент в PCM-семплы,
/// нормализованные в `f32 ∈ [-1.0, 1.0]` (независимо от `bit_depth` —
/// `hound` читает Int-семплы любой разрядности как `i32`, делим на
/// амплитуду разрядности сегмента).
fn decode_segment(seg: &TimelineSegment, key: Option<&[u8; 32]>) -> Result<Vec<f32>, PlayerError> {
    let raw = crypto::read_segment_plain(&seg.path, key)?;
    let mut reader =
        hound::WavReader::new(Cursor::new(raw)).map_err(|e| PlayerError::Decode(e.to_string()))?;
    let bits = reader.spec().bits_per_sample;
    let max_amp = (1i64 << bits.saturating_sub(1)) as f32;
    let mut out = Vec::with_capacity(reader.len() as usize);
    for s in reader.samples::<i32>() {
        let v = s.map_err(|e| PlayerError::Decode(e.to_string()))?;
        out.push(v as f32 / max_amp);
    }
    Ok(out)
}

/// Источник семплов одной дорожки: лениво декодирует сегменты таймлайна по
/// очереди, начиная с точки сика.
pub struct SegmentSource {
    channels: u16,
    sample_rate_hz: u32,
    remaining: VecDeque<TimelineSegment>,
    key: Option<[u8; 32]>,
    current: Vec<f32>,
    pos: usize,
}

impl SegmentSource {
    /// Источник без сегментов (пустой таймлайн/сик за пределы) — тишина,
    /// мгновенно завершается.
    fn empty(channels: u16, sample_rate_hz: u32) -> Self {
        Self {
            channels,
            sample_rate_hz,
            remaining: VecDeque::new(),
            key: None,
            current: Vec::new(),
            pos: 0,
        }
    }

    /// Перейти к следующему непустому декодированному сегменту очереди.
    /// `false`, если сегменты кончились (штатный конец сессии).
    fn advance_segment(&mut self) -> bool {
        while let Some(seg) = self.remaining.pop_front() {
            match decode_segment(&seg, self.key.as_ref()) {
                Ok(samples) => {
                    self.current = samples;
                    self.pos = 0;
                    return true;
                }
                Err(e) => {
                    // Повреждён/усечён сегмент не на границе сика — see
                    // module doc: останавливаем поток, а не паникуем/падаем.
                    eprintln!("[player] сегмент {:?} не декодирован: {e}", seg.path);
                    continue;
                }
            }
        }
        self.current = Vec::new();
        self.pos = 0;
        false
    }
}

impl Iterator for SegmentSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        loop {
            if self.pos < self.current.len() {
                let v = self.current[self.pos];
                self.pos += 1;
                return Some(v);
            }
            if !self.advance_segment() {
                return None;
            }
        }
    }
}

impl Source for SegmentSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate_hz
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Построить источник дорожки, начиная с абсолютного фрейма `start_frame` её
/// таймлайна (сик = пересоздание источника, а не seek внутри `Source`).
/// Первый сегмент декодируется сразу (ошибка на нём — сразу видна
/// вызывающему, см. doc модуля); остальные — лениво по мере проигрывания.
pub fn seek_source(
    timeline: &Timeline,
    start_frame: u64,
    channels: u16,
    key: Option<[u8; 32]>,
) -> Result<SegmentSource, PlayerError> {
    let Some((idx, local_offset)) = timeline.locate_index(start_frame) else {
        return Ok(SegmentSource::empty(channels, timeline.sample_rate_hz));
    };
    let mut remaining: VecDeque<TimelineSegment> =
        timeline.segments[idx..].iter().cloned().collect();
    let first = remaining
        .pop_front()
        .expect("locate_index вернул валидный индекс — сегмент существует");
    let samples = decode_segment(&first, key.as_ref())?;
    let skip = (local_offset as usize).saturating_mul(channels.max(1) as usize);
    let pos = skip.min(samples.len());
    Ok(SegmentSource {
        channels,
        sample_rate_hz: timeline.sample_rate_hz,
        remaining,
        key,
        current: samples,
        pos,
    })
}

/// Сведённый микс N дорожек (моно-источников) — среднее семплов на кадр.
/// Длина = длина кратчайшей дорожки (v1: короткие дорожки не дописываются
/// тишиной — простое, предсказуемое поведение, документируется как известное
/// упрощение).
pub struct MixSource {
    sources: Vec<SegmentSource>,
    sample_rate_hz: u32,
}

impl MixSource {
    pub fn new(sources: Vec<SegmentSource>) -> Self {
        let sample_rate_hz = sources.first().map(|s| s.sample_rate_hz).unwrap_or(0);
        Self {
            sources,
            sample_rate_hz,
        }
    }
}

impl Iterator for MixSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.sources.is_empty() {
            return None;
        }
        let mut sum = 0f32;
        for s in &mut self.sources {
            // Любая дорожка кончилась — микс кончился (длина = min).
            sum += s.next()?;
        }
        Some(sum / self.sources.len() as f32)
    }
}

impl Source for MixSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        1
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate_hz
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Обёртка источника, считающая реально проигранные фреймы (не семплы) в
/// `played_frames` — независимо от скорости воспроизведения. Оборачивает
/// источник **до** применения `Sink::set_speed` (рodio's `Speed`-адаптер
/// лишь перемаркирует заявленную частоту дискретизации для устройства вывода
/// и не меняет число вызовов `next()` на весь звук — см. doc модуля/тесты),
/// поэтому счётчик остаётся верным независимо от текущей скорости.
pub struct PositionTrackingSource<S> {
    inner: S,
    played_frames: Arc<AtomicU64>,
    channels: u16,
    channel_pos: u16,
}

impl<S> PositionTrackingSource<S> {
    pub fn new(inner: S, played_frames: Arc<AtomicU64>, channels: u16) -> Self {
        Self {
            inner,
            played_frames,
            channels: channels.max(1),
            channel_pos: 0,
        }
    }
}

impl<S: Iterator<Item = f32>> Iterator for PositionTrackingSource<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let v = self.inner.next()?;
        self.channel_pos += 1;
        if self.channel_pos >= self.channels {
            self.channel_pos = 0;
            self.played_frames.fetch_add(1, Ordering::Relaxed);
        }
        Some(v)
    }
}

impl<S> Source for PositionTrackingSource<S>
where
    S: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::manifest::SegmentRecord;
    use hound::{SampleFormat, WavSpec, WavWriter};
    use std::path::{Path, PathBuf};

    /// Записать синтетический WAV-сегмент (моно, 16 бит) с заданными семплами.
    fn write_wav(path: &Path, rate: u32, samples: &[i16]) {
        let spec = WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(path, spec).unwrap();
        for &s in samples {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    fn seg_record(track_id: u32, index: u32, path: PathBuf, frames: u64, started: u64) -> SegmentRecord {
        SegmentRecord {
            track_id,
            index,
            path: path.to_string_lossy().into_owned(),
            started_at_unix_ms: started,
            frames,
            size_bytes: frames * 2,
            sha256: String::new(),
            chain_link: String::new(),
        }
    }

    #[test]
    fn stitches_multiple_segments_into_continuous_stream() {
        let tmp = tempfile::tempdir().unwrap();
        let p1 = tmp.path().join("s1.wav");
        let p2 = tmp.path().join("s2.wav");
        write_wav(&p1, 8_000, &[1, 2, 3]);
        write_wav(&p2, 8_000, &[4, 5]);
        let records = vec![
            seg_record(0, 1, p1, 3, 1_700_000_000_000),
            seg_record(0, 2, p2, 2, 1_700_000_001_000),
        ];
        let tl = Timeline::build(&records, 0, 8_000);

        let src = seek_source(&tl, 0, 1, None).unwrap();
        let out: Vec<f32> = src.collect();
        // Нормализация: i16 / 32768.
        let expected: Vec<f32> = [1, 2, 3, 4, 5].iter().map(|&v| v as f32 / 32_768.0).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn seek_source_fails_immediately_when_first_segment_from_seek_point_is_corrupt() {
        // В отличие от повреждённого сегмента в середине потока (см.
        // `corrupt_trailing_segment_stops_stream_without_panicking`, где Iterator
        // просто останавливается), ошибка на ПЕРВОМ сегменте от точки сика
        // декодируется сразу в `seek_source` и пробрасывается вызывающему —
        // так открытие/сик на полностью нечитаемую сессию сразу видны IPC-слою.
        let tmp = tempfile::tempdir().unwrap();
        let p1 = tmp.path().join("s1-corrupt.wav");
        std::fs::write(&p1, b"not a wav file at all").unwrap();
        let records = vec![seg_record(0, 1, p1, 5, 1_700_000_000_000)];
        let tl = Timeline::build(&records, 0, 8_000);

        match seek_source(&tl, 0, 1, None) {
            Err(PlayerError::Decode(_)) => {}
            Err(PlayerError::Crypto(_)) => panic!("ожидалась ошибка декодирования, не крипто"),
            Ok(_) => panic!("ожидалась ошибка декодирования повреждённого сегмента"),
        }
    }

    #[test]
    fn seek_source_skips_exact_number_of_frames() {
        let tmp = tempfile::tempdir().unwrap();
        let p1 = tmp.path().join("s1.wav");
        write_wav(&p1, 8_000, &[10, 20, 30, 40, 50]);
        let records = vec![seg_record(0, 1, p1, 5, 1_700_000_000_000)];
        let tl = Timeline::build(&records, 0, 8_000);

        let src = seek_source(&tl, 2, 1, None).unwrap();
        let out: Vec<f32> = src.collect();
        let expected: Vec<f32> = [30, 40, 50].iter().map(|&v| v as f32 / 32_768.0).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn corrupt_trailing_segment_stops_stream_without_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let p1 = tmp.path().join("s1.wav");
        let p2 = tmp.path().join("s2-corrupt.wav");
        write_wav(&p1, 8_000, &[1, 2]);
        std::fs::write(&p2, b"not a wav file at all").unwrap();
        let records = vec![
            seg_record(0, 1, p1, 2, 1_700_000_000_000),
            seg_record(0, 2, p2, 999, 1_700_000_001_000),
        ];
        let tl = Timeline::build(&records, 0, 8_000);

        let src = seek_source(&tl, 0, 1, None).unwrap();
        let out: Vec<f32> = src.collect();
        let expected: Vec<f32> = [1, 2].iter().map(|&v| v as f32 / 32_768.0).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn multichannel_track_selection_does_not_mix_other_tracks() {
        let tmp = tempfile::tempdir().unwrap();
        let p0 = tmp.path().join("t0.wav");
        let p1 = tmp.path().join("t1.wav");
        write_wav(&p0, 8_000, &[100, 200]);
        write_wav(&p1, 8_000, &[900, 800]);
        let records = vec![
            seg_record(0, 1, p0, 2, 1_700_000_000_000),
            seg_record(1, 1, p1, 2, 1_700_000_000_000),
        ];
        let tl1 = Timeline::build(&records, 1, 8_000);
        let src = seek_source(&tl1, 0, 1, None).unwrap();
        let out: Vec<f32> = src.collect();
        let expected: Vec<f32> = [900, 800].iter().map(|&v| v as f32 / 32_768.0).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn mix_source_averages_two_tracks() {
        let tmp = tempfile::tempdir().unwrap();
        let p0 = tmp.path().join("t0.wav");
        let p1 = tmp.path().join("t1.wav");
        write_wav(&p0, 8_000, &[100, 200, 300]);
        write_wav(&p1, 8_000, &[300, 200, 100]);
        let records = vec![
            seg_record(0, 1, p0, 3, 1_700_000_000_000),
            seg_record(1, 1, p1, 3, 1_700_000_000_000),
        ];
        let tl0 = Timeline::build(&records, 0, 8_000);
        let tl1 = Timeline::build(&records, 1, 8_000);
        let s0 = seek_source(&tl0, 0, 1, None).unwrap();
        let s1 = seek_source(&tl1, 0, 1, None).unwrap();
        let mix = MixSource::new(vec![s0, s1]);
        let out: Vec<f32> = mix.collect();
        assert_eq!(out.len(), 3);
        for v in &out {
            assert!((*v - 200.0 / 32_768.0).abs() < 1e-6);
        }
    }

    #[test]
    fn mix_source_length_is_shortest_track() {
        let tmp = tempfile::tempdir().unwrap();
        let p0 = tmp.path().join("t0.wav");
        let p1 = tmp.path().join("t1.wav");
        write_wav(&p0, 8_000, &[1, 2, 3, 4]);
        write_wav(&p1, 8_000, &[1, 2]);
        let records = vec![
            seg_record(0, 1, p0, 4, 1_700_000_000_000),
            seg_record(1, 1, p1, 2, 1_700_000_000_000),
        ];
        let tl0 = Timeline::build(&records, 0, 8_000);
        let tl1 = Timeline::build(&records, 1, 8_000);
        let s0 = seek_source(&tl0, 0, 1, None).unwrap();
        let s1 = seek_source(&tl1, 0, 1, None).unwrap();
        let mix = MixSource::new(vec![s0, s1]);
        assert_eq!(mix.count(), 2);
    }

    #[test]
    fn position_tracking_counts_frames_independent_of_external_speed_control() {
        // PositionTrackingSource считает исходные фреймы источника — обёртка
        // не знает о скорости воспроизведения (та применяется отдельно, вне
        // этого адаптера, см. doc модуля) и потому не влияет на счётчик.
        let samples = vec![0.1_f32, 0.2, 0.3, 0.4, 0.5, 0.6]; // 2 канала × 3 фрейма
        let played = Arc::new(AtomicU64::new(0));
        let src = PositionTrackingSource::new(samples.into_iter(), Arc::clone(&played), 2);
        let out: Vec<f32> = src.collect();
        assert_eq!(out.len(), 6);
        assert_eq!(played.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn position_tracking_partial_frame_at_end_not_counted() {
        // Нечётное число семплов при 2 каналах: последний неполный кадр не
        // засчитывается (симметрично тому, как считает `next()`).
        let samples = vec![0.1_f32, 0.2, 0.3];
        let played = Arc::new(AtomicU64::new(0));
        let src = PositionTrackingSource::new(samples.into_iter(), Arc::clone(&played), 2);
        let _out: Vec<f32> = src.collect();
        assert_eq!(played.load(Ordering::Relaxed), 1);
    }
}
