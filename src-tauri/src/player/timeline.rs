//! Таймлайн сессии/дорожки: склейка сегментов манифеста в непрерывную ось
//! фреймов + сопоставление wall-clock (меток разметки, этап `10`) с этой
//! осью (этап 10.1 — `promts/10_1_playback.md`, шаг 1).
//!
//! Модуль **без I/O**: только модель и арифметика, тестируется отдельно от
//! декодера ([`super::source`]) и Tauri-слоя.
//!
//! Смещения меток (`integrity::annotations::sample_offset`/`ms_offset`)
//! считаются от wall-clock старта сессии, а не от оси реально записанных
//! фреймов: во время паузы захват не пишет семплы (`audio::capture`, без
//! zero-fill), поэтому ось фреймов короче wall-clock оси на суммарную
//! длительность пауз. [`Timeline::frame_at_wall_clock`] учитывает это через
//! реальный `started_at_unix_ms` каждого сегмента (реконсилируется из
//! журнала — `store::reconcile`), а не через наивный пересчёт офсета в
//! семплы по общей частоте.

use std::path::PathBuf;

use crate::store::manifest::SegmentRecord;

/// Один сегмент дорожки в составе таймлайна: путь на диске + позиция на
/// непрерывной оси фреймов дорожки + реальный wall-clock интервал записи.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineSegment {
    pub track_id: u32,
    pub index: u32,
    pub path: PathBuf,
    /// Смещение начала сегмента на оси фреймов дорожки (сумма `frames`
    /// предыдущих сегментов).
    pub start_frame: u64,
    pub frames: u64,
    /// Реальное время открытия сегмента (мс от эпохи Unix).
    pub started_at_unix_ms: u64,
}

/// Непрерывная ось фреймов одной дорожки сессии.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub segments: Vec<TimelineSegment>,
    pub total_frames: u64,
    pub sample_rate_hz: u32,
}

impl Timeline {
    /// Построить таймлайн дорожки `track_id` из сегментов манифеста
    /// (`ManifestStore::get_segments`, все дорожки вперемешку). Усечённый
    /// последний сегмент (обрыв питания — этап 02) — обычный сегмент с
    /// меньшим `frames`, ничего специально не делаем: длина берётся из
    /// факта, зафиксированного в манифесте при реконсиляции.
    pub fn build(segments: &[SegmentRecord], track_id: u32, sample_rate_hz: u32) -> Timeline {
        let mut filtered: Vec<&SegmentRecord> =
            segments.iter().filter(|s| s.track_id == track_id).collect();
        filtered.sort_by_key(|s| s.index);

        let mut out = Vec::with_capacity(filtered.len());
        let mut cursor = 0u64;
        for s in filtered {
            out.push(TimelineSegment {
                track_id: s.track_id,
                index: s.index,
                path: PathBuf::from(&s.path),
                start_frame: cursor,
                frames: s.frames,
                started_at_unix_ms: s.started_at_unix_ms,
            });
            cursor += s.frames;
        }

        Timeline {
            total_frames: cursor,
            segments: out,
            sample_rate_hz,
        }
    }

    /// Индекс сегмента (в `self.segments`), содержащего абсолютный `frame` оси
    /// дорожки, + смещение внутри него. Клэмп к последнему валидному фрейму.
    pub fn locate_index(&self, frame: u64) -> Option<(usize, u64)> {
        if self.segments.is_empty() {
            return None;
        }
        let clamped = frame.min(self.total_frames.saturating_sub(1));
        let idx = match self
            .segments
            .binary_search_by(|s| s.start_frame.cmp(&clamped))
        {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        };
        Some((idx, clamped - self.segments[idx].start_frame))
    }

    /// Найти сегмент, содержащий абсолютный `frame` оси дорожки, + смещение
    /// внутри него. Клэмп к последнему валидному фрейму, если `frame`
    /// выходит за пределы (пустой таймлайн — `None`).
    pub fn locate(&self, frame: u64) -> Option<(&TimelineSegment, u64)> {
        let (idx, off) = self.locate_index(frame)?;
        Some((&self.segments[idx], off))
    }

    /// Абсолютный фрейм оси дорожки, соответствующий моменту `at_unix_ms`.
    /// Если момент попадает в «дыру» между сегментами (пауза записи) —
    /// клэмп к концу предыдущего сегмента (последний реально записанный
    /// момент); момент раньше первого сегмента — клэмп к 0; момент позже
    /// последнего — клэмп к последнему фрейму.
    pub fn frame_at_wall_clock(&self, at_unix_ms: u64) -> u64 {
        if self.segments.is_empty() || self.sample_rate_hz == 0 {
            return 0;
        }
        let at = at_unix_ms as u128;
        let rate = self.sample_rate_hz as u128;
        let mut prev_end_frame = 0u64;
        for seg in &self.segments {
            let seg_start_ms = seg.started_at_unix_ms as u128;
            let seg_duration_ms = (seg.frames as u128) * 1000 / rate;
            let seg_end_ms = seg_start_ms + seg_duration_ms;
            if at < seg_start_ms {
                return prev_end_frame;
            }
            if at < seg_end_ms {
                let within_ms = at - seg_start_ms;
                let within_frames = (within_ms * rate) / 1000;
                return seg.start_frame + within_frames as u64;
            }
            prev_end_frame = seg.start_frame + seg.frames.saturating_sub(1);
        }
        self.total_frames.saturating_sub(1)
    }

    /// Абсолютный фрейм оси дорожки для метки с офсетом `marker_offset_ms`
    /// от старта сессии `session_started_at_unix_ms` (та же ось, что
    /// `integrity::annotations::MarkerState.offset_ms`).
    pub fn frame_for_marker(&self, session_started_at_unix_ms: u64, marker_offset_ms: u64) -> u64 {
        self.frame_at_wall_clock(session_started_at_unix_ms.saturating_add(marker_offset_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(track_id: u32, index: u32, frames: u64, started_at_unix_ms: u64) -> SegmentRecord {
        SegmentRecord {
            track_id,
            index,
            path: format!("seg-{index:04}.wav"),
            started_at_unix_ms,
            frames,
            size_bytes: frames * 2,
            sha256: String::new(),
            chain_link: String::new(),
        }
    }

    #[test]
    fn build_orders_and_accumulates_start_frames() {
        // Специально подаём сегменты не по порядку — build должен отсортировать.
        let segs = vec![
            seg(0, 2, 4_410, 1_700_000_030_000),
            seg(0, 1, 44_100, 1_700_000_000_000),
        ];
        let tl = Timeline::build(&segs, 0, 44_100);
        assert_eq!(tl.segments.len(), 2);
        assert_eq!(tl.segments[0].index, 1);
        assert_eq!(tl.segments[0].start_frame, 0);
        assert_eq!(tl.segments[1].index, 2);
        assert_eq!(tl.segments[1].start_frame, 44_100);
        assert_eq!(tl.total_frames, 44_100 + 4_410);
    }

    #[test]
    fn build_filters_by_track_id() {
        let segs = vec![
            seg(0, 1, 100, 1_700_000_000_000),
            seg(1, 1, 200, 1_700_000_000_000),
        ];
        let tl0 = Timeline::build(&segs, 0, 44_100);
        let tl1 = Timeline::build(&segs, 1, 44_100);
        assert_eq!(tl0.total_frames, 100);
        assert_eq!(tl1.total_frames, 200);
    }

    #[test]
    fn build_handles_truncated_last_segment_without_panicking() {
        // Обрыв питания: последний сегмент короче штатной длины сегмента.
        let segs = vec![
            seg(0, 1, 44_100, 1_700_000_000_000),
            seg(0, 2, 137, 1_700_000_030_000), // усечён
        ];
        let tl = Timeline::build(&segs, 0, 44_100);
        assert_eq!(tl.total_frames, 44_100 + 137);
        assert_eq!(tl.segments[1].frames, 137);
    }

    #[test]
    fn locate_finds_segment_and_inner_offset() {
        let segs = vec![
            seg(0, 1, 100, 1_700_000_000_000),
            seg(0, 2, 50, 1_700_000_010_000),
        ];
        let tl = Timeline::build(&segs, 0, 44_100);
        let (s, off) = tl.locate(0).unwrap();
        assert_eq!(s.index, 1);
        assert_eq!(off, 0);
        let (s, off) = tl.locate(99).unwrap();
        assert_eq!(s.index, 1);
        assert_eq!(off, 99);
        let (s, off) = tl.locate(100).unwrap();
        assert_eq!(s.index, 2);
        assert_eq!(off, 0);
        let (s, off) = tl.locate(149).unwrap();
        assert_eq!(s.index, 2);
        assert_eq!(off, 49);
    }

    #[test]
    fn locate_clamps_beyond_total_frames() {
        let segs = vec![seg(0, 1, 100, 1_700_000_000_000)];
        let tl = Timeline::build(&segs, 0, 44_100);
        let (s, off) = tl.locate(10_000).unwrap();
        assert_eq!(s.index, 1);
        assert_eq!(off, 99);
    }

    #[test]
    fn locate_on_empty_timeline_is_none() {
        let tl = Timeline::build(&[], 0, 44_100);
        assert!(tl.locate(0).is_none());
    }

    #[test]
    fn frame_at_wall_clock_matches_naive_without_pause() {
        // Два сегмента ровно встык (нет паузы): 44100 Гц, 1с сегмент.
        let segs = vec![
            seg(0, 1, 44_100, 1_700_000_000_000),
            seg(0, 2, 44_100, 1_700_000_001_000),
        ];
        let tl = Timeline::build(&segs, 0, 44_100);
        // Момент 1.5с от старта сессии = кадр 66150.
        assert_eq!(tl.frame_at_wall_clock(1_700_000_001_500), 66_150);
        // Точно старт второго сегмента.
        assert_eq!(tl.frame_at_wall_clock(1_700_000_001_000), 44_100);
    }

    #[test]
    fn frame_at_wall_clock_clamps_across_pause_gap() {
        // Регрессия этапа 10.1: между сегментами — пауза 10с (второй сегмент
        // стартует не сразу после конца первого). Метка, поставленная в
        // разгар паузы, не должна наивно проецироваться в середину второго
        // сегмента — она должна клэмпиться к концу первого (последний
        // реально записанный момент).
        let segs = vec![
            seg(0, 1, 44_100, 1_700_000_000_000), // конец: 1_700_000_001_000
            seg(0, 2, 44_100, 1_700_000_011_000), // пауза 10с перед стартом
        ];
        let tl = Timeline::build(&segs, 0, 44_100);
        // Момент в середине паузы (1_700_000_005_000) — клэмп к концу сегмента 1.
        let frame = tl.frame_at_wall_clock(1_700_000_005_000);
        assert_eq!(frame, 44_099); // последний фрейм первого сегмента
        // Наивный (неверный) пересчёт дал бы кадр далеко во втором сегменте.
        assert!(frame < tl.segments[1].start_frame);
    }

    #[test]
    fn frame_at_wall_clock_clamps_before_first_and_after_last() {
        let segs = vec![seg(0, 1, 44_100, 1_700_000_000_000)];
        let tl = Timeline::build(&segs, 0, 44_100);
        assert_eq!(tl.frame_at_wall_clock(1_699_999_999_000), 0);
        assert_eq!(tl.frame_at_wall_clock(1_700_000_005_000), 44_099);
    }

    #[test]
    fn frame_for_marker_adds_session_start_and_offset() {
        let segs = vec![
            seg(0, 1, 44_100, 1_700_000_000_000),
            seg(0, 2, 44_100, 1_700_000_001_000),
        ];
        let tl = Timeline::build(&segs, 0, 44_100);
        // Метка на 1.5с сессии.
        assert_eq!(tl.frame_for_marker(1_700_000_000_000, 1_500), 66_150);
    }
}
