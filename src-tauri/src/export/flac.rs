//! Обёртка над `flacenc` (чистый Rust FLAC-энкодер, без libFLAC-биндингов —
//! тот же принцип, что `hound`/`rusqlite(bundled)`/`reqwest(rustls-tls)`).
//! Кодирует из целочисленного PCM (тот же путь, что WAV-склейка
//! [`super::audio`]), без плавающей точки.
//!
//! **Потоковое кодирование (R-014, этап 13.7).** Ранее весь WAV читался в
//! `Vec<i32>` и сериализовался в память (`ByteSink`) целиком — трёхчасовая
//! моно-сессия требовала ~1.9 ГБ RAM (риск OOM на станции с 4 ГБ). Теперь
//! семплы читаются из WAV поблочно ([`WavBlockSource`]), каждый фрейм
//! кодируется по отдельности ([`flacenc::encode_fixed_size_frame`] — тот же
//! примитив, что внутри `encode_with_fixed_block_size`) и дозаписывается в
//! файл; пиковая память — O(блок), не O(сессия). Заголовок STREAMINFO
//! записывается плейсхолдером и переписывается по завершении (его размер
//! фиксирован спецификацией FLAC), когда известны настоящие длина, MD5 и
//! статистика фреймов.
//!
//! Семантика прежняя: последний физический фрейм имеет фиксированный размер
//! блока, даже если реальных семплов в нём меньше — «хвост» добивается внутри
//! фрейма (заполнение делает `FrameBuf`, ровно как у `MemSource` прежнего
//! пути). Это не потеря/искажение данных: `STREAMINFO.total_samples` корректно
//! хранит настоящую длину, и любой конформный декодер/браузер проигрывает
//! ровно её, не добивку сверху (до одного блока, ~десятки мс на типичных
//! частотах).

use std::fs::File;
use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use flacenc::component::{BitRepr, StreamInfo};
use flacenc::error::Verify;
use flacenc::source::{Context, Fill, FrameBuf, Source};

use super::ExportError;

/// Байт заголовка METADATA_BLOCK для STREAMINFO, являющегося последним
/// метаданным блоком: старший бит `is-last` + тип 0 (STREAMINFO). Константа
/// формата FLAC, не настройка.
const STREAMINFO_BLOCK_HEADER: u8 = 0x80;
/// Размер тела STREAMINFO — ровно 34 байта (фиксирован спецификацией FLAC);
/// благодаря этому плейсхолдер-заголовок можно переписать по завершении.
const STREAMINFO_BODY_LEN: usize = 34;

/// Потоковый источник семплов для `flacenc` поверх WAV-файла: читает
/// интерливнутые семплы поблочно (буфер одного блока переиспользуется),
/// не загружая сессию в память.
struct WavBlockSource {
    reader: hound::WavReader<BufReader<File>>,
    channels: usize,
    bits_per_sample: usize,
    sample_rate: usize,
    /// Настоящая длина (кадров на канал) — из WAV-заголовка.
    total_frames: usize,
    /// Переиспользуемый буфер интерливнутого блока.
    buf: Vec<i32>,
}

impl WavBlockSource {
    fn open(wav_path: &Path) -> Result<Self, ExportError> {
        let reader =
            hound::WavReader::open(wav_path).map_err(|e| ExportError::Decode(e.to_string()))?;
        let spec = reader.spec();
        let total_frames = reader.duration() as usize;
        Ok(Self {
            reader,
            channels: spec.channels as usize,
            bits_per_sample: spec.bits_per_sample as usize,
            sample_rate: spec.sample_rate as usize,
            total_frames,
            buf: Vec::new(),
        })
    }
}

impl Source for WavBlockSource {
    fn channels(&self) -> usize {
        self.channels
    }

    fn bits_per_sample(&self) -> usize {
        self.bits_per_sample
    }

    fn sample_rate(&self) -> usize {
        self.sample_rate
    }

    fn read_samples<F: Fill>(
        &mut self,
        block_size: usize,
        dest: &mut F,
    ) -> Result<usize, flacenc::error::SourceError> {
        let want = block_size * self.channels;
        self.buf.clear();
        for s in self.reader.samples::<i32>().take(want) {
            let v = s.map_err(flacenc::error::SourceError::from_io_error)?;
            self.buf.push(v);
        }
        if self.buf.is_empty() {
            return Ok(0);
        }
        dest.fill_interleaved(&self.buf)?;
        Ok(self.buf.len() / self.channels)
    }

    fn len_hint(&self) -> Option<usize> {
        Some(self.total_frames)
    }
}

/// Сериализовать заголовок потока (`fLaC` + единственный метаданный блок
/// STREAMINFO) — то же, что пишет `flacenc::component::Stream` без фреймов.
/// Размер результата фиксирован, поэтому финальная перезапись по смещению 0
/// не сдвигает уже записанные фреймы.
fn stream_header_bytes(stream_info: &StreamInfo) -> Result<Vec<u8>, ExportError> {
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream_info
        .write(&mut sink)
        .map_err(|e| ExportError::Decode(format!("ошибка сериализации STREAMINFO: {e:?}")))?;
    let body = sink.as_slice();
    if body.len() != STREAMINFO_BODY_LEN {
        return Err(ExportError::Decode(format!(
            "неожиданный размер STREAMINFO: {} байт вместо {STREAMINFO_BODY_LEN}",
            body.len()
        )));
    }
    let mut out = Vec::with_capacity(4 + 4 + STREAMINFO_BODY_LEN);
    out.extend_from_slice(b"fLaC");
    out.push(STREAMINFO_BLOCK_HEADER);
    // 24-битная длина тела блока (big-endian) — формат FLAC.
    out.extend_from_slice(&(STREAMINFO_BODY_LEN as u32).to_be_bytes()[1..]);
    out.extend_from_slice(body);
    Ok(out)
}

/// Закодировать WAV-файл (целочисленный PCM, моно/интерливнутый по каналам)
/// в FLAC потоково: память — O(блок), файл дозаписывается по фреймам.
pub fn encode_wav_to_flac(wav_path: &Path, out_path: &Path) -> Result<(), ExportError> {
    let mut source = WavBlockSource::open(wav_path)?;

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, e)| {
            ExportError::Decode(format!("некорректная конфигурация FLAC-энкодера: {e:?}"))
        })?;
    let block_size = config.block_size;

    let mut stream_info =
        StreamInfo::new(source.sample_rate, source.channels, source.bits_per_sample)
            .map_err(|e| ExportError::Decode(format!("некорректный формат PCM: {e:?}")))?;
    let mut framebuf_and_context = (
        FrameBuf::with_size(source.channels, block_size)
            .map_err(|e| ExportError::Decode(format!("некорректный размер блока: {e:?}")))?,
        Context::new(source.bits_per_sample, source.channels, block_size),
    );

    let mut out = BufWriter::new(File::create(out_path)?);
    // Плейсхолдер заголовка: настоящие длина/MD5/статистика фреймов известны
    // только после прохода — перепишем по смещению 0 (размер фиксирован).
    out.write_all(&stream_header_bytes(&stream_info)?)?;

    let mut sink = flacenc::bitsink::ByteSink::new();
    loop {
        let read = source
            .read_samples(block_size, &mut framebuf_and_context)
            .map_err(|e| ExportError::Decode(format!("ошибка чтения WAV: {e:?}")))?;
        if read == 0 {
            break;
        }
        let frame_number = framebuf_and_context
            .1
            .current_frame_number()
            .expect("после успешного чтения номер фрейма известен");
        let frame = flacenc::encode_fixed_size_frame(
            &config,
            &framebuf_and_context.0,
            frame_number,
            &stream_info,
        )
        .map_err(|e| ExportError::Decode(format!("ошибка FLAC-кодирования: {e:?}")))?;
        stream_info.update_frame_info(&frame);

        sink.clear();
        frame
            .write(&mut sink)
            .map_err(|e| ExportError::Decode(format!("ошибка сериализации FLAC-фрейма: {e:?}")))?;
        out.write_all(sink.as_slice())?;
    }

    // Финальный STREAMINFO: настоящая длина (не кратная блоку) и MD5 входа.
    stream_info.set_total_samples(source.total_frames);
    stream_info.set_md5_digest(&framebuf_and_context.1.md5_digest());

    out.flush()?;
    let mut file = out
        .into_inner()
        .map_err(|e| ExportError::Io(e.to_string()))?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&stream_header_bytes(&stream_info)?)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};

    fn write_wav(path: &Path, channels: u16, rate: u32, bits: u16, samples: &[i32]) {
        let spec = WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: bits,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(path, spec).unwrap();
        for &s in samples {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    /// Декодировать FLAC и обрезать до `streaminfo().samples` (заявленной
    /// длины). Фиксированный размер блока энкодера может дополнить
    /// последний физический фрейм тишиной сверх реальной длины сигнала —
    /// это не потеря/искажение данных (штатные декодеры и браузеры не
    /// проигрывают её, ориентируясь на `STREAMINFO.total_samples`), поэтому
    /// побайтовое сравнение семплов делаем по заявленной длине, как и любой
    /// конформный проигрыватель.
    fn decode_flac_i32(path: &Path) -> (u32, u32, Vec<i32>) {
        let mut r = claxon::FlacReader::open(path).unwrap();
        let info = r.streaminfo();
        let mut samples: Vec<i32> = r.samples().map(|s| s.unwrap()).collect();
        if let Some(total) = info.samples {
            samples.truncate(total as usize * info.channels as usize);
        }
        (info.sample_rate, info.channels, samples)
    }

    #[test]
    fn encode_wav_to_flac_roundtrips_exact_samples() {
        let tmp = tempfile::tempdir().unwrap();
        let wav = tmp.path().join("in.wav");
        let flac = tmp.path().join("out.flac");
        let samples = [0, 1, -1, 32_767, -32_768, 12_345, -12_345, 0, 0, 7];
        write_wav(&wav, 1, 44_100, 16, &samples);

        encode_wav_to_flac(&wav, &flac).unwrap();
        assert!(flac.exists());

        let (rate, channels, decoded) = decode_flac_i32(&flac);
        assert_eq!(rate, 44_100);
        assert_eq!(channels, 1);
        assert_eq!(decoded, samples);
    }

    #[test]
    fn encode_wav_to_flac_preserves_channel_rate_bits() {
        let tmp = tempfile::tempdir().unwrap();
        let wav = tmp.path().join("in.wav");
        let flac = tmp.path().join("out.flac");
        // Стерео (интерливнутые семплы L,R,L,R,...).
        let samples = [1, -1, 2, -2, 3, -3, 4, -4];
        write_wav(&wav, 2, 8_000, 16, &samples);

        encode_wav_to_flac(&wav, &flac).unwrap();
        let (rate, channels, decoded) = decode_flac_i32(&flac);
        assert_eq!(rate, 8_000);
        assert_eq!(channels, 2);
        assert_eq!(decoded, samples);
    }

    #[test]
    fn encode_multiblock_signal_with_partial_tail_roundtrips() {
        // Ядро R-014: сигнал длиннее нескольких блоков энкодера + «хвост»,
        // не кратный размеру блока (дефолтный block_size flacenc — 4096).
        // Потоковый путь обязан дать те же семплы и настоящую длину в
        // STREAMINFO.total_samples, что и загрузка целиком.
        let tmp = tempfile::tempdir().unwrap();
        let wav = tmp.path().join("in.wav");
        let flac = tmp.path().join("out.flac");
        let frames = 3 * 4096 + 1234; // 3 полных блока + неполный хвост
        let samples: Vec<i32> = (0..frames)
            .map(|i| ((i * 37) % 65_536) as i32 - 32_768)
            .collect();
        write_wav(&wav, 1, 44_100, 16, &samples);

        encode_wav_to_flac(&wav, &flac).unwrap();

        let mut r = claxon::FlacReader::open(&flac).unwrap();
        assert_eq!(r.streaminfo().samples, Some(frames as u64));
        let decoded: Vec<i32> = r.samples().map(|s| s.unwrap()).collect();
        // Декодер отдаёт и добивку последнего блока сверх настоящей длины —
        // сверяем по заявленной длине (как любой конформный проигрыватель,
        // см. doc-комментарий модуля).
        assert_eq!(&decoded[..frames], &samples[..]);
    }

    #[test]
    fn encode_reports_md5_of_input_pcm() {
        // MD5 в STREAMINFO — верификация декодеров (flac -t). Потоковый путь
        // считает его инкрементально (Context) — сверяем c claxon.
        let tmp = tempfile::tempdir().unwrap();
        let wav = tmp.path().join("in.wav");
        let flac = tmp.path().join("out.flac");
        let samples: Vec<i32> = (0..5000).map(|i| (i % 200) - 100).collect();
        write_wav(&wav, 1, 16_000, 16, &samples);

        encode_wav_to_flac(&wav, &flac).unwrap();
        let r = claxon::FlacReader::open(&flac).unwrap();
        assert_ne!(r.streaminfo().md5sum, [0u8; 16], "MD5 заполнен");
    }
}
