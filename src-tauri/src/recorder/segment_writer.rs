//! Сегментный WAV-райтер (этап 01 — `promts/01_audio_core.md`, шаг 5).
//!
//! Потребитель кадров записи: пишет PCM-семплы в WAV короткими сегментами с
//! **непрерывным сбросом на диск** и ротацией. Ядро «бесперебойности»:
//! - ротация каждые `recorder.segment_seconds` (единица файлов и — позже —
//!   хеширования);
//! - fsync каждые `recorder.flush_interval_ms` (`writer.flush()` обновляет
//!   заголовок + `File::sync_all` сбрасывает данные на диск) — максимальная
//!   потеря при сбое питания не превышает этот интервал, а усечённый сегмент
//!   остаётся читаемым.
//!
//! Без ресемпла: WAV пишется на **фактической** частоте устройства. Все
//! числовые параметры приходят снаружи (из `Settings`) — магических чисел нет.
//!
//! Полный манифест сессии (БД, хеш-цепочка) — этап 03; здесь манифест —
//! заглушка: накопленный список [`SegmentInfo`].

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::audio::AudioError;

/// Параметры сегментного райтера (из `Settings`).
#[derive(Debug, Clone)]
pub struct SegmentConfig {
    /// Корневой каталог для сегментов сессии.
    pub dir: PathBuf,
    /// Фактическая частота записи (нативная частота устройства).
    pub sample_rate_hz: u32,
    /// Число каналов мастера (`audio.channels`).
    pub channels: u16,
    /// Разрядность PCM (`audio.bit_depth`).
    pub bits_per_sample: u16,
    /// Длина сегмента (`recorder.segment_seconds`).
    pub segment_seconds: u32,
    /// Интервал fsync (`recorder.flush_interval_ms`).
    pub flush_interval: Duration,
}

impl SegmentConfig {
    /// Порог ротации в кадрах = `segment_seconds × sample_rate_hz`.
    fn segment_frame_limit(&self) -> u64 {
        self.segment_seconds as u64 * self.sample_rate_hz as u64
    }

    fn wav_spec(&self) -> WavSpec {
        WavSpec {
            channels: self.channels,
            sample_rate: self.sample_rate_hz,
            bits_per_sample: self.bits_per_sample,
            sample_format: SampleFormat::Int,
        }
    }
}

/// Описание записанного сегмента (заглушка манифеста до этапа 03).
#[derive(Debug, Clone)]
pub struct SegmentInfo {
    pub index: u32,
    pub path: PathBuf,
    /// Число записанных кадров (семплов на канал).
    pub frames: u64,
    /// Время старта сегмента (Unix-миллисекунды) — таймкод имени файла.
    pub started_at_unix_ms: u128,
}

/// Открытый текущий сегмент: hound-райтер поверх `BufWriter<File>` + клон файла
/// для честного `sync_all` (hound владеет writer'ом и сам fsync не делает).
struct OpenSegment {
    writer: WavWriter<BufWriter<File>>,
    fsync_handle: File,
    info: SegmentInfo,
}

/// Сегментный райтер: один на сессию записи.
pub struct SegmentWriter {
    config: SegmentConfig,
    frame_limit: u64,
    next_index: u32,
    current: Option<OpenSegment>,
    segments: Vec<SegmentInfo>,
    /// Курсор: сколько завершённых сегментов уже отдано через [`SegmentWriter::drain_completed`].
    drained_upto: usize,
    last_flush: Instant,
}

impl SegmentWriter {
    /// Создать райтер; каталог сессии создаётся при необходимости.
    pub fn new(config: SegmentConfig) -> Result<Self, AudioError> {
        std::fs::create_dir_all(&config.dir).map_err(|e| AudioError::Io(e.to_string()))?;
        let frame_limit = config.segment_frame_limit().max(1);
        Ok(Self {
            config,
            frame_limit,
            next_index: 1,
            current: None,
            segments: Vec::new(),
            drained_upto: 0,
            last_flush: Instant::now(),
        })
    }

    /// Записать интерливнутые PCM-семплы (`channels` семплов на кадр). Делает
    /// ротацию по достижении порога сегмента; не аллоцирует на стационарном
    /// режиме сверх hound-буфера.
    pub fn write_samples(&mut self, samples: &[i16]) -> Result<(), AudioError> {
        if samples.is_empty() {
            return Ok(());
        }
        let channels = self.config.channels.max(1) as usize;
        // Пишем покадрово, чтобы ротация попадала ровно на границу кадра.
        for frame in samples.chunks_exact(channels) {
            if self.current.is_none() {
                self.open_new_segment()?;
            }
            // `current` гарантированно Some после open_new_segment.
            let seg = self.current.as_mut().expect("сегмент открыт");
            for &s in frame {
                seg.writer
                    .write_sample(s)
                    .map_err(|e| AudioError::Io(e.to_string()))?;
            }
            seg.info.frames += 1;
            if seg.info.frames >= self.frame_limit {
                self.rotate()?;
            }
        }
        Ok(())
    }

    /// Сбросить буферы на диск (fsync), если истёк `flush_interval`. Вызывается
    /// потребителем периодически; ограничивает потерю при сбое питания.
    pub fn maybe_flush(&mut self) -> Result<(), AudioError> {
        if self.last_flush.elapsed() >= self.config.flush_interval {
            self.flush_now()?;
        }
        Ok(())
    }

    /// Безусловный сброс на диск.
    pub fn flush_now(&mut self) -> Result<(), AudioError> {
        if let Some(seg) = self.current.as_mut() {
            seg.writer
                .flush()
                .map_err(|e| AudioError::Io(e.to_string()))?;
            seg.fsync_handle
                .sync_all()
                .map_err(|e| AudioError::Io(e.to_string()))?;
        }
        self.last_flush = Instant::now();
        Ok(())
    }

    /// Вернуть сегменты, **завершённые с прошлого вызова** (ротацией или
    /// финализацией). Потребитель опрашивает этот метод, чтобы по факту закрытия
    /// сегмента журналировать его и зеркалировать (этап 02). Не аллоцирует, пока
    /// нет новых завершённых сегментов.
    pub fn drain_completed(&mut self) -> Vec<SegmentInfo> {
        if self.drained_upto >= self.segments.len() {
            return Vec::new();
        }
        let drained = self.segments[self.drained_upto..].to_vec();
        self.drained_upto = self.segments.len();
        drained
    }

    /// Завершить запись: финализировать текущий сегмент и вернуть список всех
    /// записанных сегментов (заглушка манифеста).
    pub fn finalize(mut self) -> Result<Vec<SegmentInfo>, AudioError> {
        self.close_current()?;
        Ok(self.segments)
    }

    fn open_new_segment(&mut self) -> Result<(), AudioError> {
        let started_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let index = self.next_index;
        let path = self
            .config
            .dir
            .join(segment_file_name(index, started_at_unix_ms));

        let file = File::create(&path).map_err(|e| AudioError::Io(e.to_string()))?;
        let fsync_handle = file
            .try_clone()
            .map_err(|e| AudioError::Io(e.to_string()))?;
        let writer = WavWriter::new(BufWriter::new(file), self.config.wav_spec())
            .map_err(|e| AudioError::Io(e.to_string()))?;

        self.current = Some(OpenSegment {
            writer,
            fsync_handle,
            info: SegmentInfo {
                index,
                path,
                frames: 0,
                started_at_unix_ms,
            },
        });
        self.next_index += 1;
        self.last_flush = Instant::now();
        Ok(())
    }

    /// Закрыть текущий сегмент (если открыт), зафиксировав его в списке.
    fn close_current(&mut self) -> Result<(), AudioError> {
        if let Some(seg) = self.current.take() {
            let info = seg.info.clone();
            let fsync_handle = seg.fsync_handle;
            seg.writer
                .finalize()
                .map_err(|e| AudioError::Io(e.to_string()))?;
            // Финальный fsync, чтобы корректный заголовок гарантированно лёг на диск.
            fsync_handle
                .sync_all()
                .map_err(|e| AudioError::Io(e.to_string()))?;
            self.segments.push(info);
        }
        Ok(())
    }

    fn rotate(&mut self) -> Result<(), AudioError> {
        self.close_current()
    }

    /// Снимок уже завершённых сегментов (для диагностики/тестов).
    #[cfg(test)]
    fn finished_segments(&self) -> &[SegmentInfo] {
        &self.segments
    }
}

/// Имя файла сегмента: порядковый № (4 знака) + Unix-таймкод (мс).
fn segment_file_name(index: u32, unix_ms: u128) -> String {
    format!("seg-{index:04}-{unix_ms}.wav")
}

/// Является ли путь WAV-сегментом этого райтера (для тестов/диагностики).
#[allow(dead_code)]
fn is_segment_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with("seg-") && n.ends_with(".wav"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(dir: PathBuf, rate: u32, segment_seconds: u32) -> SegmentConfig {
        SegmentConfig {
            dir,
            sample_rate_hz: rate,
            channels: 1,
            bits_per_sample: 16,
            segment_seconds,
            flush_interval: Duration::from_millis(1_500),
        }
    }

    #[test]
    fn writes_valid_wav_header() {
        let tmp = tempfile::tempdir().unwrap();
        let rate = 44_100;
        let mut w = SegmentWriter::new(cfg(tmp.path().to_path_buf(), rate, 30)).unwrap();
        let samples: Vec<i16> = (0..1000).map(|i| (i % 100) as i16).collect();
        w.write_samples(&samples).unwrap();
        let segs = w.finalize().unwrap();

        assert_eq!(segs.len(), 1);
        let reader = hound::WavReader::open(&segs[0].path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, rate);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, SampleFormat::Int);
        assert_eq!(reader.len() as usize, samples.len());
    }

    #[test]
    fn rotates_on_segment_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        // rate=10, segment=1s -> порог 10 кадров. 25 кадров -> 3 сегмента (10,10,5).
        let mut w = SegmentWriter::new(cfg(tmp.path().to_path_buf(), 10, 1)).unwrap();
        let samples: Vec<i16> = (0..25).map(|i| i as i16).collect();
        w.write_samples(&samples).unwrap();

        // Два сегмента уже закрыты ротацией до финализации.
        assert_eq!(w.finished_segments().len(), 2);
        let segs = w.finalize().unwrap();
        assert_eq!(segs.len(), 3);

        let lens: Vec<u32> = segs
            .iter()
            .map(|s| hound::WavReader::open(&s.path).unwrap().len())
            .collect();
        assert_eq!(lens, vec![10, 10, 5]);
        // Непрерывность: сумма кадров = всем записанным семплам, без потерь.
        assert_eq!(lens.iter().sum::<u32>() as usize, samples.len());
    }

    #[test]
    fn drain_completed_yields_each_segment_once() {
        let tmp = tempfile::tempdir().unwrap();
        // rate=10, segment=1s -> порог 10 кадров.
        let mut w = SegmentWriter::new(cfg(tmp.path().to_path_buf(), 10, 1)).unwrap();

        // Первые 10 кадров закрывают сегмент №1 ротацией.
        w.write_samples(&(0..10).map(|i| i as i16).collect::<Vec<_>>())
            .unwrap();
        let first = w.drain_completed();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].index, 1);
        // Повторный дрейн без новых завершений — пусто.
        assert!(w.drain_completed().is_empty());

        // Ещё 10 кадров -> сегмент №2; 5 кадров остаются в открытом №3.
        w.write_samples(&(0..15).map(|i| i as i16).collect::<Vec<_>>())
            .unwrap();
        let second = w.drain_completed();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].index, 2);

        // Финализация закрывает №3; finalize отдаёт ВСЕ сегменты.
        let all = w.finalize().unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn segment_names_are_indexed() {
        assert_eq!(
            segment_file_name(1, 1700000000000),
            "seg-0001-1700000000000.wav"
        );
        assert_eq!(segment_file_name(42, 0), "seg-0042-0.wav");
    }

    #[test]
    fn empty_write_creates_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = SegmentWriter::new(cfg(tmp.path().to_path_buf(), 44_100, 30)).unwrap();
        w.write_samples(&[]).unwrap();
        let segs = w.finalize().unwrap();
        assert!(segs.is_empty());
        let count = std::fs::read_dir(tmp.path()).unwrap().count();
        assert_eq!(count, 0);
    }
}
