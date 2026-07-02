//! Обёртка над `flacenc` (чистый Rust FLAC-энкодер, без libFLAC-биндингов —
//! тот же принцип, что `hound`/`rusqlite(bundled)`/`reqwest(rustls-tls)`).
//! Кодирует из целочисленного PCM (тот же путь, что WAV-склейка
//! [`super::audio`]), без плавающей точки.
//!
//! `encode_with_fixed_block_size` пишет последний физический фрейм
//! фиксированного размера блока, даже если реальных семплов в нём меньше —
//! «хвост» дополняется тишиной внутри фрейма. Это не потеря/искажение
//! данных: `STREAMINFO.total_samples` корректно хранит настоящую длину, и
//! любой конформный декодер/браузер проигрывает ровно её, не лишнюю
//! тишину сверху (до одного блока, ~десятки мс на типичных частотах).

use std::path::Path;

use flacenc::component::BitRepr;
use flacenc::error::Verify;

use super::ExportError;

/// Закодировать WAV-файл (целочисленный PCM, моно/интерливнутый по каналам)
/// в FLAC.
pub fn encode_wav_to_flac(wav_path: &Path, out_path: &Path) -> Result<(), ExportError> {
    let mut reader =
        hound::WavReader::open(wav_path).map_err(|e| ExportError::Decode(e.to_string()))?;
    let spec = reader.spec();
    let samples: Vec<i32> = reader
        .samples::<i32>()
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| ExportError::Decode(e.to_string()))?;

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, e)| ExportError::Decode(format!("некорректная конфигурация FLAC-энкодера: {e:?}")))?;
    let source = flacenc::source::MemSource::from_samples(
        &samples,
        spec.channels as usize,
        spec.bits_per_sample as usize,
        spec.sample_rate as usize,
    );
    let block_size = config.block_size;
    let flac_stream = flacenc::encode_with_fixed_block_size(&config, source, block_size)
        .map_err(|e| ExportError::Decode(format!("ошибка FLAC-кодирования: {e:?}")))?;

    let mut sink = flacenc::bitsink::ByteSink::new();
    flac_stream
        .write(&mut sink)
        .map_err(|e| ExportError::Decode(format!("ошибка сериализации FLAC-потока: {e:?}")))?;
    std::fs::write(out_path, sink.as_slice())?;
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
}
