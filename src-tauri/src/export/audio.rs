//! Побайтовая (без f32-нормализации) дешифровка+склейка сегментов таймлайна в
//! один WAV-файл (этап 10.2, шаг 1).
//!
//! В отличие от [`crate::player::source::decode_segment`] (10.1) — который
//! нормализует семплы в `f32` для живого прослушивания и **тихо обрывает**
//! поток на повреждённом сегменте — здесь семплы остаются целочисленными
//! (без потери точности) и любая ошибка сегмента — жёсткий `Err`: выдаваемая
//! копия не может тихо потерять хвост записи.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Cursor};
use std::path::Path;

use hound::{WavReader, WavSpec, WavWriter};

use crate::player::timeline::{Timeline, TimelineSegment};
use crate::store::crypto;

use super::ExportError;

/// Итог склейки дорожки (или микса) в один WAV-файл.
#[derive(Debug)]
pub struct JoinedTrack {
    /// Число кадров (семплов на канал) в результирующем файле.
    pub frames: u64,
    pub spec: WavSpec,
}

/// Дешифровать один сегмент и вернуть его формат PCM + целочисленные семплы
/// (без f32-прохода).
fn decode_segment_samples(
    seg: &TimelineSegment,
    key: Option<&[u8; 32]>,
) -> Result<(WavSpec, Vec<i32>), ExportError> {
    let raw = crypto::read_segment_plain(&seg.path, key)?;
    let mut reader = WavReader::new(Cursor::new(raw))
        .map_err(|e| ExportError::Decode(format!("{}: {e}", seg.path.display())))?;
    let spec = reader.spec();
    let samples: Vec<i32> = reader
        .samples::<i32>()
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| ExportError::Decode(format!("{}: {e}", seg.path.display())))?;
    Ok((spec, samples))
}

/// Лениво декодирующий целочисленный курсор по сегментам одной дорожки —
/// целочисленный аналог `player::source::SegmentSource`, но без тихого
/// обрыва: ошибка сегмента возвращается вызывающему, а не проглатывается.
struct IntSegmentCursor<'a> {
    remaining: VecDeque<&'a TimelineSegment>,
    key: Option<&'a [u8; 32]>,
    current: Vec<i32>,
    pos: usize,
    spec: Option<WavSpec>,
}

impl<'a> IntSegmentCursor<'a> {
    fn new(timeline: &'a Timeline, key: Option<&'a [u8; 32]>) -> Self {
        Self {
            remaining: timeline.segments.iter().collect(),
            key,
            current: Vec::new(),
            pos: 0,
            spec: None,
        }
    }

    /// Следующий целочисленный семпл; `None` — дорожка кончилась (штатно);
    /// `Some(Err(_))` — жёсткая ошибка сегмента.
    fn next_sample(&mut self) -> Option<Result<i32, ExportError>> {
        loop {
            if self.pos < self.current.len() {
                let v = self.current[self.pos];
                self.pos += 1;
                return Some(Ok(v));
            }
            let seg = self.remaining.pop_front()?;
            match decode_segment_samples(seg, self.key) {
                Ok((spec, samples)) => {
                    if let Some(prev) = self.spec {
                        if prev != spec {
                            return Some(Err(ExportError::Decode(format!(
                                "сегмент {} имеет другой формат PCM, чем предыдущие сегменты дорожки ({:?} vs {:?})",
                                seg.path.display(),
                                spec,
                                prev
                            ))));
                        }
                    } else {
                        self.spec = Some(spec);
                    }
                    self.current = samples;
                    self.pos = 0;
                }
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

/// Дешифровать+склеить сегменты `timeline` в один WAV-файл `out_path`
/// (целочисленный PCM, без f32-прохода). Ошибка на любом сегменте — жёсткий
/// `Err`, не тихое усечение (см. doc модуля).
pub fn join_track_to_wav(
    timeline: &Timeline,
    key: Option<&[u8; 32]>,
    out_path: &Path,
) -> Result<JoinedTrack, ExportError> {
    if timeline.segments.is_empty() {
        return Err(ExportError::Decode("дорожка не содержит сегментов".into()));
    }

    let mut cursor = IntSegmentCursor::new(timeline, key);
    let mut writer: Option<WavWriter<BufWriter<File>>> = None;
    let mut samples_written = 0u64;

    loop {
        match cursor.next_sample() {
            Some(Ok(v)) => {
                if writer.is_none() {
                    let spec = cursor.spec.expect("spec установлен при декодировании сегмента");
                    writer = Some(
                        WavWriter::create(out_path, spec).map_err(|e| ExportError::Io(e.to_string()))?,
                    );
                }
                writer
                    .as_mut()
                    .expect("создан выше")
                    .write_sample(v)
                    .map_err(|e| ExportError::Io(e.to_string()))?;
                samples_written += 1;
            }
            Some(Err(e)) => return Err(e),
            None => break,
        }
    }

    let spec = cursor
        .spec
        .ok_or_else(|| ExportError::Decode("дорожка не содержит семплов".into()))?;
    let writer = writer.ok_or_else(|| ExportError::Decode("дорожка не содержит семплов".into()))?;
    writer.finalize().map_err(|e| ExportError::Io(e.to_string()))?;

    let frames = if spec.channels > 0 {
        samples_written / spec.channels as u64
    } else {
        0
    };
    Ok(JoinedTrack { frames, spec })
}

/// Свести N моно-дорожек в один WAV поэлементным целочисленным усреднением
/// (округление к ближайшему). Длина результата = длина кратчайшей дорожки —
/// то же ограничение, что у `player::source::MixSource`, задокументированное
/// как известное упрощение. Дорожки обязаны иметь одинаковый формат PCM
/// (частота/разрядность/канальность) — иначе жёсткая ошибка.
pub fn join_mix_to_wav(
    timelines: &[&Timeline],
    key: Option<&[u8; 32]>,
    out_path: &Path,
) -> Result<JoinedTrack, ExportError> {
    if timelines.is_empty() {
        return Err(ExportError::Decode("микс требует хотя бы одну дорожку".into()));
    }
    if timelines.iter().any(|tl| tl.segments.is_empty()) {
        return Err(ExportError::Decode(
            "одна из дорожек микса не содержит сегментов".into(),
        ));
    }

    let mut cursors: Vec<IntSegmentCursor> = timelines
        .iter()
        .map(|tl| IntSegmentCursor::new(tl, key))
        .collect();

    let n = cursors.len() as i64;
    let mut writer: Option<WavWriter<BufWriter<File>>> = None;
    let mut out_spec: Option<WavSpec> = None;
    let mut samples_written = 0u64;

    'outer: loop {
        let mut sum: i64 = 0;
        for c in &mut cursors {
            match c.next_sample() {
                Some(Ok(v)) => sum += v as i64,
                Some(Err(e)) => return Err(e),
                None => break 'outer,
            }
        }

        if samples_written == 0 {
            // Все дорожки только что декодировали свой первый сегмент —
            // сверяем формат PCM один раз, а не на каждый семпл.
            let mut common: Option<WavSpec> = None;
            for c in &cursors {
                let spec = c.spec.expect("установлен первым next_sample выше");
                match common {
                    None => common = Some(spec),
                    Some(prev) if prev != spec => {
                        return Err(ExportError::Decode(
                            "дорожки микса имеют разный формат PCM".into(),
                        ))
                    }
                    _ => {}
                }
            }
            let spec = common.expect("хотя бы одна дорожка есть — common установлен");
            let mono_spec = WavSpec {
                channels: 1,
                ..spec
            };
            writer = Some(
                WavWriter::create(out_path, mono_spec)
                    .map_err(|e| ExportError::Io(e.to_string()))?,
            );
            out_spec = Some(mono_spec);
        }

        let avg = ((sum as f64) / (n as f64)).round() as i32;
        writer
            .as_mut()
            .expect("создан на первой итерации")
            .write_sample(avg)
            .map_err(|e| ExportError::Io(e.to_string()))?;
        samples_written += 1;
    }

    let spec = out_spec.ok_or_else(|| ExportError::Decode("микс не содержит семплов".into()))?;
    let writer = writer.ok_or_else(|| ExportError::Decode("микс не содержит семплов".into()))?;
    writer.finalize().map_err(|e| ExportError::Io(e.to_string()))?;

    Ok(JoinedTrack {
        frames: samples_written,
        spec,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::manifest::SegmentRecord;
    use hound::{SampleFormat, WavSpec as Spec, WavWriter as Writer};
    use std::path::PathBuf;

    fn write_wav(path: &Path, rate: u32, bits: u16, samples: &[i32]) {
        let spec = Spec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: bits,
            sample_format: SampleFormat::Int,
        };
        let mut w = Writer::create(path, spec).unwrap();
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

    fn read_i32_samples(path: &Path) -> Vec<i32> {
        let mut r = WavReader::open(path).unwrap();
        r.samples::<i32>().map(|s| s.unwrap()).collect()
    }

    #[test]
    fn join_track_matches_original_samples_unencrypted() {
        let tmp = tempfile::tempdir().unwrap();
        let p1 = tmp.path().join("s1.wav");
        let p2 = tmp.path().join("s2.wav");
        write_wav(&p1, 8_000, 16, &[1, 2, 3]);
        write_wav(&p2, 8_000, 16, &[4, 5]);
        let records = vec![
            seg_record(0, 1, p1, 3, 1_700_000_000_000),
            seg_record(0, 2, p2, 2, 1_700_000_001_000),
        ];
        let tl = Timeline::build(&records, 0, 8_000);

        let out = tmp.path().join("out.wav");
        let joined = join_track_to_wav(&tl, None, &out).unwrap();
        assert_eq!(joined.frames, 5);
        assert_eq!(read_i32_samples(&out), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn join_track_matches_original_samples_encrypted() {
        // Побайтовое совпадение декодированного аудио с оригиналом ДО
        // шифрования — прямой критерий приёмки промта.
        let tmp = tempfile::tempdir().unwrap();
        let plain = tmp.path().join("s1.wav");
        write_wav(&plain, 44_100, 16, &[100, -100, 32_767, -32_768, 0]);
        let original_samples = read_i32_samples(&plain);
        let original_hash = crate::integrity::hash::sha256_file(&plain).unwrap();

        let key = [7u8; 32];
        let fin = crypto::finalize_segment(&plain, Some(&key), true).unwrap();
        assert!(fin.encrypted);

        let records = vec![seg_record(0, 1, fin.stored_path.clone(), 5, 1_700_000_000_000)];
        let tl = Timeline::build(&records, 0, 44_100);

        let out = tmp.path().join("out.wav");
        join_track_to_wav(&tl, Some(&key), &out).unwrap();
        assert_eq!(read_i32_samples(&out), original_samples);
        assert_eq!(
            crate::integrity::hash::sha256_file(&out).unwrap(),
            original_hash
        );
    }

    #[test]
    fn join_track_errors_hard_on_corrupt_segment_instead_of_truncating() {
        // Расхождение с `player::source`: там повреждённый НЕ-первый сегмент
        // тихо обрывает поток; здесь — жёсткая ошибка (копия не может
        // незаметно потерять хвост записи).
        let tmp = tempfile::tempdir().unwrap();
        let p1 = tmp.path().join("s1.wav");
        let p2 = tmp.path().join("s2-corrupt.wav");
        write_wav(&p1, 8_000, 16, &[1, 2]);
        std::fs::write(&p2, b"not a wav file at all").unwrap();
        let records = vec![
            seg_record(0, 1, p1, 2, 1_700_000_000_000),
            seg_record(0, 2, p2, 999, 1_700_000_001_000),
        ];
        let tl = Timeline::build(&records, 0, 8_000);

        let out = tmp.path().join("out.wav");
        let err = join_track_to_wav(&tl, None, &out).unwrap_err();
        assert!(matches!(err, ExportError::Decode(_)));
    }

    #[test]
    fn join_track_errors_without_key_when_segment_encrypted() {
        let tmp = tempfile::tempdir().unwrap();
        let plain = tmp.path().join("s1.wav");
        write_wav(&plain, 8_000, 16, &[1, 2, 3]);
        let key = [3u8; 32];
        let fin = crypto::finalize_segment(&plain, Some(&key), true).unwrap();

        let records = vec![seg_record(0, 1, fin.stored_path, 3, 1_700_000_000_000)];
        let tl = Timeline::build(&records, 0, 8_000);

        let out = tmp.path().join("out.wav");
        let err = join_track_to_wav(&tl, None, &out).unwrap_err();
        assert!(matches!(err, ExportError::Crypto(_)));
    }

    #[test]
    fn join_track_errors_on_empty_timeline() {
        let tmp = tempfile::tempdir().unwrap();
        let tl = Timeline::build(&[], 0, 8_000);
        let out = tmp.path().join("out.wav");
        assert!(join_track_to_wav(&tl, None, &out).is_err());
    }

    #[test]
    fn join_mix_averages_two_tracks_with_integer_rounding() {
        let tmp = tempfile::tempdir().unwrap();
        let p0 = tmp.path().join("t0.wav");
        let p1 = tmp.path().join("t1.wav");
        write_wav(&p0, 8_000, 16, &[100, 200, 300]);
        write_wav(&p1, 8_000, 16, &[300, 201, 100]);
        let r0 = vec![seg_record(0, 1, p0, 3, 1_700_000_000_000)];
        let r1 = vec![seg_record(1, 1, p1, 3, 1_700_000_000_000)];
        let tl0 = Timeline::build(&r0, 0, 8_000);
        let tl1 = Timeline::build(&r1, 1, 8_000);

        let out = tmp.path().join("mix.wav");
        let joined = join_mix_to_wav(&[&tl0, &tl1], None, &out).unwrap();
        assert_eq!(joined.frames, 3);
        assert_eq!(joined.spec.channels, 1);
        // (100+300)/2=200, (200+201)/2=200.5→округление к 201(round half away from zero), (300+100)/2=200
        assert_eq!(read_i32_samples(&out), vec![200, 201, 200]);
    }

    #[test]
    fn join_mix_length_is_shortest_track() {
        let tmp = tempfile::tempdir().unwrap();
        let p0 = tmp.path().join("t0.wav");
        let p1 = tmp.path().join("t1.wav");
        write_wav(&p0, 8_000, 16, &[1, 2, 3, 4]);
        write_wav(&p1, 8_000, 16, &[1, 2]);
        let r0 = vec![seg_record(0, 1, p0, 4, 1_700_000_000_000)];
        let r1 = vec![seg_record(1, 1, p1, 2, 1_700_000_000_000)];
        let tl0 = Timeline::build(&r0, 0, 8_000);
        let tl1 = Timeline::build(&r1, 1, 8_000);

        let out = tmp.path().join("mix.wav");
        let joined = join_mix_to_wav(&[&tl0, &tl1], None, &out).unwrap();
        assert_eq!(joined.frames, 2);
    }

    #[test]
    fn join_mix_rejects_mismatched_specs() {
        let tmp = tempfile::tempdir().unwrap();
        let p0 = tmp.path().join("t0.wav");
        let p1 = tmp.path().join("t1.wav");
        write_wav(&p0, 8_000, 16, &[1, 2, 3]);
        write_wav(&p1, 16_000, 16, &[1, 2, 3]); // другая частота
        let r0 = vec![seg_record(0, 1, p0, 3, 1_700_000_000_000)];
        let r1 = vec![seg_record(1, 1, p1, 3, 1_700_000_000_000)];
        let tl0 = Timeline::build(&r0, 0, 8_000);
        let tl1 = Timeline::build(&r1, 1, 16_000);

        let out = tmp.path().join("mix.wav");
        let err = join_mix_to_wav(&[&tl0, &tl1], None, &out).unwrap_err();
        assert!(matches!(err, ExportError::Decode(_)));
    }

    #[test]
    fn join_track_errors_on_mismatched_spec_between_segments() {
        let tmp = tempfile::tempdir().unwrap();
        let p1 = tmp.path().join("s1.wav");
        let p2 = tmp.path().join("s2.wav");
        write_wav(&p1, 8_000, 16, &[1, 2]);
        write_wav(&p2, 8_000, 8, &[1, 2]); // другая разрядность
        let records = vec![
            seg_record(0, 1, p1, 2, 1_700_000_000_000),
            seg_record(0, 2, p2, 2, 1_700_000_001_000),
        ];
        let tl = Timeline::build(&records, 0, 8_000);

        let out = tmp.path().join("out.wav");
        let err = join_track_to_wav(&tl, None, &out).unwrap_err();
        assert!(matches!(err, ExportError::Decode(_)));
    }
}
