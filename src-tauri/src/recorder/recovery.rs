//! Восстановление незавершённой сессии при старте — этап 02
//! (`promts/02_recorder_reliability.md`, deliverable 3).
//!
//! При запуске приложения сканируем корень хранилища, по журналу
//! ([`super::journal`]) находим сессии без штатной записи `Stopped` и чиним
//! последний (усечённый сбоем питания) сегмент: PCM-данные WAV остаются
//! читаемыми, но размеры в RIFF/`data`-заголовке не соответствуют фактической
//! длине файла. Переписываем заголовок по факту и отбрасываем неполный
//! хвостовой кадр — потеря не превышает `flush_interval_ms`.
//!
//! Решение заказчика: по умолчанию **дописываем ту же сессию**, помечая её
//! `Recovered`; финальный выбор (продолжить / закрыть) оставлен оператору в UI
//! (этап 04). Здесь — обнаружение, починка и пометка.

use std::path::{Path, PathBuf};

use super::journal::{self, JournalRecord, SessionMeta};
use crate::store::crypto;

/// Незавершённая сессия, найденная при сканировании.
#[derive(Debug, Clone)]
pub struct UnfinishedSession {
    /// Каталог сессии (содержит журнал и сегменты).
    pub dir: PathBuf,
    /// Метаданные формата (из `SessionStarted`), если присутствуют.
    pub meta: Option<SessionMeta>,
    /// Завершённые по журналу сегменты.
    pub completed_segments: Vec<journal::CompletedSegment>,
    /// Уже была помечена восстановленной ранее.
    pub recovered: bool,
}

/// Итог починки одного WAV-сегмента.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairOutcome {
    /// Заголовок уже соответствует данным — правка не потребовалась.
    AlreadyValid,
    /// Заголовок переписан; в файле теперь `data_bytes` корректных байт данных.
    Repaired { data_bytes: u64 },
    /// Починить нельзя (файл короче валидного заголовка/нет `data`-чанка).
    Unrepairable(String),
}

/// Просканировать корень хранилища и вернуть незавершённые сессии (каталоги с
/// журналом, в котором нет штатной записи `Stopped`).
pub fn scan_unfinished(root: &Path) -> std::io::Result<Vec<UnfinishedSession>> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let journal_path = dir.join(journal::JOURNAL_FILE_NAME);
        if !journal_path.exists() {
            continue;
        }
        let state = journal::replay(&journal_path)?;
        if state.is_unfinished() {
            out.push(UnfinishedSession {
                dir,
                meta: state.started,
                completed_segments: state.completed_segments,
                recovered: state.recovered,
            });
        }
    }
    Ok(out)
}

/// Дописать в журнал сессии запись `Recovered` (идемпотентность — на стороне
/// читателя: повторная пометка безвредна).
pub fn mark_recovered(dir: &Path) -> std::io::Result<()> {
    let mut j = journal::Journal::open(dir)?;
    j.append(&JournalRecord::Recovered)
}

/// Найти файлы сегментов сессии (`seg-*.wav` и — при шифровании at-rest,
/// R-013 — `seg-*.wav.enc`), отсортированные по имени (= по возрастанию
/// индекса, т.к. индекс дополнен нулями).
pub fn segment_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("seg-") && (n.ends_with(".wav") || n.ends_with(".wav.enc")))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    Ok(files)
}

/// Восстановить сессию «на месте»: починить последний (возможно усечённый)
/// сегмент, **дожурналить** сегменты, не успевшие получить `SegmentCompleted`
/// (при крахе это хвостовой сегмент — его запись в журнал делается только при
/// закрытии), дофинализировать их в `.enc` при включённом шифровании
/// (`encryption_key = Some`, R-013) и пометить сессию `Recovered`. Возвращает
/// итог починки последнего сегмента (или `None`, если сегментов нет).
///
/// `encryption_key`: `Some` — политика `storage.encrypt_at_rest` активна, ключ
/// станции разрешён (fail-secure гейт — на вызывающей стороне); `None` —
/// plaintext-режим, файлы остаются WAV.
pub fn recover_in_place(
    dir: &Path,
    encryption_key: Option<&[u8; crypto::KEY_LEN]>,
) -> std::io::Result<Option<RepairOutcome>> {
    let segments = segment_files(dir)?;
    // Чинится только незашифрованный хвост: `.enc`-сегменты финализированы
    // целиком (шифрование идёт после закрытия файла) и в починке не нуждаются.
    let outcome = match segments.last() {
        Some(last) if !is_encrypted_segment(last) => Some(repair_last_segment(last)?),
        _ => None,
    };
    journal_stray_segments(dir, &segments, encryption_key)?;
    mark_recovered(dir)?;
    Ok(outcome)
}

/// Сегмент хранится шифрованным (`….wav.enc`)?
fn is_encrypted_segment(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e == crypto::ENCRYPTED_EXT)
        .unwrap_or(false)
}

/// Дожурналить сегменты, лежащие на диске без записи `SegmentCompleted`
/// (закрытие сегмента журналируется по факту финализации, которой при крахе не
/// происходит), и — при включённом шифровании — дофинализировать открытые WAV в
/// `.enc`. Без этого хвостовой сегмент не попадал бы в манифест/хеш-цепочку/
/// выгрузку (потеря последних ≤ `recorder.segment_seconds` записи).
fn journal_stray_segments(
    dir: &Path,
    segments: &[PathBuf],
    encryption_key: Option<&[u8; crypto::KEY_LEN]>,
) -> std::io::Result<()> {
    let journal_path = dir.join(journal::JOURNAL_FILE_NAME);
    if !journal_path.exists() {
        return Ok(());
    }
    let state = journal::replay(&journal_path)?;
    let known: std::collections::HashSet<String> = state
        .completed_segments
        .iter()
        .map(|s| canonical_segment_name(&s.path))
        .collect();

    let mut j = journal::Journal::open(dir)?;
    for path in segments {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if known.contains(&canonical_segment_name(&name)) {
            continue;
        }
        // Метаданные сегмента — из имени файла (`seg-NNNN-<unix_ms>.wav[.enc]`)
        // и фактического содержимого (кадры — из WAV-заголовка).
        let Some((index, started_at_unix_ms)) = parse_segment_name(&name) else {
            continue;
        };
        let Some(frames) = segment_frames(path, encryption_key) else {
            continue; // нечитаемый файл — не журналируем битую запись
        };
        // Дофинализация (R-013): хеш и шифрование делает `finalize_segment`;
        // хеш здесь не персистится — реконсиляция пересчитает его по
        // каноничному содержимому (`read_segment_plain`).
        let stored_name = match encryption_key {
            Some(key) if !is_encrypted_segment(path) => {
                match crypto::finalize_segment(path, Some(key), true) {
                    Ok(fin) => fin
                        .stored_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&name)
                        .to_string(),
                    Err(e) => {
                        eprintln!(
                            "[recovery] ВНИМАНИЕ: дофинализация {path:?} не удалась ({e}) — сегмент оставлен открытым WAV"
                        );
                        name.clone()
                    }
                }
            }
            _ => name.clone(),
        };
        j.append(&JournalRecord::SegmentCompleted {
            index,
            path: stored_name,
            frames,
            started_at_unix_ms,
        })?;
    }
    Ok(())
}

/// Каноничное имя сегмента без суффикса шифрования: `X.wav.enc` и `X.wav`
/// обозначают один и тот же сегмент.
fn canonical_segment_name(name: &str) -> String {
    name.strip_suffix(&format!(".{}", crypto::ENCRYPTED_EXT))
        .unwrap_or(name)
        .to_string()
}

/// Разобрать имя файла сегмента `seg-NNNN-<unix_ms>.wav[.enc]` → (индекс, таймкод).
fn parse_segment_name(name: &str) -> Option<(u32, u64)> {
    let stem = canonical_segment_name(name);
    let rest = stem.strip_prefix("seg-")?.strip_suffix(".wav")?;
    let (index, unix_ms) = rest.split_once('-')?;
    Some((index.parse().ok()?, unix_ms.parse().ok()?))
}

/// Число кадров сегмента по WAV-заголовку (для `.enc` — после дешифрования).
fn segment_frames(path: &Path, key: Option<&[u8; crypto::KEY_LEN]>) -> Option<u64> {
    if is_encrypted_segment(path) {
        let plain = crypto::read_segment_plain(path, key).ok()?;
        let reader = hound::WavReader::new(std::io::Cursor::new(plain)).ok()?;
        Some(reader.duration() as u64)
    } else {
        let reader = hound::WavReader::open(path).ok()?;
        Some(reader.duration() as u64)
    }
}

// ── Починка WAV-заголовка ──────────────────────────────────────────────────────

fn read_u32_le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn read_u16_le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// Позиции чанков `fmt ` и `data` (смещения их 4-байтных идентификаторов).
struct WavLayout {
    fmt_pos: usize,
    data_pos: usize,
}

/// Пройти RIFF-чанки и найти `fmt `/`data`. Терпит неверный размер `data`-чанка
/// (как раз он и портится при усечении): идентификатор находим напрямую.
fn locate_chunks(bytes: &[u8]) -> Result<WavLayout, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("не WAV/RIFF-файл или заголовок усечён".into());
    }
    let mut fmt_pos = None;
    let mut pos = 12usize;
    // Чанки до `data` (в частности `fmt `) записаны целиком ещё при создании,
    // поэтому их размеры корректны и обход безопасен.
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        if id == b"data" {
            return Ok(WavLayout {
                fmt_pos: fmt_pos.ok_or("чанк fmt не найден до data")?,
                data_pos: pos,
            });
        }
        if id == b"fmt " {
            fmt_pos = Some(pos);
        }
        let size = read_u32_le(bytes, pos + 4) as usize;
        // RIFF выравнивает чанки до чётной длины.
        pos += 8 + size + (size & 1);
    }
    Err("чанк data не найден".into())
}

/// Починить усечённый WAV-сегмент по фактической длине файла.
///
/// Алгоритм: найти `data`-чанк, вычислить число фактических байт данных, обрезать
/// до целого числа кадров (`block_align` из `fmt`), переписать размер `data`-чанка
/// и общий размер RIFF, усечь файл до конца данных. Идемпотентно: на корректном
/// файле возвращает [`RepairOutcome::AlreadyValid`].
pub fn repair_last_segment(path: &Path) -> std::io::Result<RepairOutcome> {
    let bytes = std::fs::read(path)?;

    let layout = match locate_chunks(&bytes) {
        Ok(l) => l,
        Err(e) => return Ok(RepairOutcome::Unrepairable(e)),
    };

    // block_align = numChannels * bitsPerSample/8 (поле в fmt по смещению +20 от id).
    let block_align = read_u16_le(&bytes, layout.fmt_pos + 20) as usize;
    if block_align == 0 {
        return Ok(RepairOutcome::Unrepairable("block_align = 0 в fmt".into()));
    }

    let data_content_start = layout.data_pos + 8;
    if data_content_start > bytes.len() {
        return Ok(RepairOutcome::Unrepairable(
            "файл короче заголовка data-чанка".into(),
        ));
    }

    let actual_data_bytes = bytes.len() - data_content_start;
    // Отбрасываем неполный хвостовой кадр (потеря ≤ flush-интервала).
    let aligned_data_bytes = (actual_data_bytes / block_align) * block_align;

    let declared_data_bytes = read_u32_le(&bytes, layout.data_pos + 4) as usize;
    let new_riff_size = (data_content_start + aligned_data_bytes - 8) as u32;
    let declared_riff_size = read_u32_le(&bytes, 4);

    // Уже валиден: размеры совпадают и нет неполного хвостового кадра.
    if declared_data_bytes == aligned_data_bytes
        && declared_riff_size == new_riff_size
        && aligned_data_bytes == actual_data_bytes
    {
        return Ok(RepairOutcome::AlreadyValid);
    }

    let mut fixed = bytes;
    fixed[4..8].copy_from_slice(&new_riff_size.to_le_bytes());
    let data_size = aligned_data_bytes as u32;
    fixed[layout.data_pos + 4..layout.data_pos + 8].copy_from_slice(&data_size.to_le_bytes());
    fixed.truncate(data_content_start + aligned_data_bytes);

    std::fs::write(path, &fixed)?;
    Ok(RepairOutcome::Repaired {
        data_bytes: aligned_data_bytes as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::segment_writer::{SegmentConfig, SegmentWriter};
    use std::path::PathBuf;
    use std::time::Duration;

    fn write_wav(dir: &Path, rate: u32, frames: usize) -> PathBuf {
        let cfg = SegmentConfig {
            dir: dir.to_path_buf(),
            sample_rate_hz: rate,
            channels: 1,
            bits_per_sample: 16,
            segment_seconds: 3600, // без ротации в тесте
            flush_interval: Duration::from_millis(1_500),
        };
        let mut w = SegmentWriter::new(cfg).unwrap();
        let samples: Vec<i16> = (0..frames).map(|i| (i % 100) as i16).collect();
        w.write_samples(&samples).unwrap();
        let segs = w.finalize().unwrap();
        segs[0].path.clone()
    }

    #[test]
    fn valid_wav_needs_no_repair() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_wav(tmp.path(), 8_000, 1_000);
        assert_eq!(
            repair_last_segment(&path).unwrap(),
            RepairOutcome::AlreadyValid
        );
    }

    #[test]
    fn repairs_zeroed_data_size_header() {
        // Имитация сбоя до flush: данные на диске, но размеры в заголовке = 0.
        let tmp = tempfile::tempdir().unwrap();
        let path = write_wav(tmp.path(), 8_000, 1_000);
        let mut bytes = std::fs::read(&path).unwrap();
        let layout = locate_chunks(&bytes).unwrap();
        bytes[4..8].copy_from_slice(&0u32.to_le_bytes()); // RIFF size = 0
        bytes[layout.data_pos + 4..layout.data_pos + 8].copy_from_slice(&0u32.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();

        // hound не открывает «нулевой» заголовок как 1000 семплов.
        let before = hound::WavReader::open(&path).unwrap().len();
        assert_eq!(before, 0);

        let outcome = repair_last_segment(&path).unwrap();
        assert!(matches!(outcome, RepairOutcome::Repaired { .. }));

        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.len(), 1_000);
    }

    #[test]
    fn drops_incomplete_trailing_frame() {
        // 16-бит стерео: block_align = 4 байта. Обрежем на 2 байта (полкадра) —
        // починка должна отбросить неполный кадр и оставить целое число кадров.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SegmentConfig {
            dir: tmp.path().to_path_buf(),
            sample_rate_hz: 8_000,
            channels: 2,
            bits_per_sample: 16,
            segment_seconds: 3600,
            flush_interval: Duration::from_millis(1_500),
        };
        let mut w = SegmentWriter::new(cfg).unwrap();
        // 10 кадров стерео = 20 i16.
        let samples: Vec<i16> = (0..20).map(|i| i as i16).collect();
        w.write_samples(&samples).unwrap();
        let path = w.finalize().unwrap()[0].path.clone();

        // Усекаем файл на 2 байта (половина последнего кадра) и портим размеры.
        let mut bytes = std::fs::read(&path).unwrap();
        let layout = locate_chunks(&bytes).unwrap();
        bytes.truncate(bytes.len() - 2);
        bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
        bytes[layout.data_pos + 4..layout.data_pos + 8].copy_from_slice(&0u32.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();

        let outcome = repair_last_segment(&path).unwrap();
        // 10 кадров минус неполный хвост = 9 целых кадров (9*4 = 36 байт).
        assert_eq!(outcome, RepairOutcome::Repaired { data_bytes: 36 });
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.len(), 18); // 9 кадров × 2 канала
    }

    #[test]
    fn unrepairable_when_no_riff_header() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broken.wav");
        std::fs::write(&path, b"junk").unwrap();
        assert!(matches!(
            repair_last_segment(&path).unwrap(),
            RepairOutcome::Unrepairable(_)
        ));
    }

    #[test]
    fn scan_finds_unfinished_session() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Незавершённая сессия: журнал без Stopped.
        let unfinished = root.join("session-1");
        let mut j = journal::Journal::open(&unfinished).unwrap();
        j.append(&JournalRecord::SessionStarted {
            started_at_unix_ms: 1,
            sample_rate_hz: 8_000,
            channels: 1,
            bit_depth: 16,
            segment_seconds: 30,
            operator_id: String::new(),
            station_id: String::new(),
            autonomous_offline: false,
        })
        .unwrap();
        // Завершённая сессия: журнал со Stopped.
        let finished = root.join("session-2");
        let mut j2 = journal::Journal::open(&finished).unwrap();
        j2.append(&JournalRecord::SessionStarted {
            started_at_unix_ms: 2,
            sample_rate_hz: 8_000,
            channels: 1,
            bit_depth: 16,
            segment_seconds: 30,
            operator_id: String::new(),
            station_id: String::new(),
            autonomous_offline: false,
        })
        .unwrap();
        j2.append(&JournalRecord::Stopped).unwrap();

        let found = scan_unfinished(root).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].dir, unfinished);

        // mark_recovered делает сессию помеченной.
        mark_recovered(&found[0].dir).unwrap();
        let state = journal::replay(&unfinished.join(journal::JOURNAL_FILE_NAME)).unwrap();
        assert!(state.recovered);
    }

    #[test]
    fn scan_missing_root_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        assert!(scan_unfinished(&missing).unwrap().is_empty());
    }
}
