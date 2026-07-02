//! Аварийно-устойчивый журнал сессии (write-ahead) — этап 02
//! (`promts/02_recorder_reliability.md`, deliverable 2).
//!
//! Append-only файл `session.journal` в каталоге сессии. Формат — **JSONL**:
//! одна запись на строку (`serde_json`). При ключевых событиях (старт сессии,
//! завершение сегмента, пауза/возобновление, обрыв устройства, стоп) делаем
//! `fsync` (`File::sync_all`), чтобы после внезапного завершения процесса
//! однозначно восстановить состояние сессии и список готовых сегментов.
//!
//! Граница с этапом 03: здесь — **минимальный** журнал восстановления. Полный
//! манифест с SHA-256/хеш-цепочкой и шифрованием at-rest расширит этот же
//! журнал на этапе 03.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::integrity::annotations::AnnotationRecord;

/// Имя файла журнала в каталоге сессии.
pub const JOURNAL_FILE_NAME: &str = "session.journal";

/// Запись журнала. Тег `type` (`snake_case`) делает строки самоописываемыми.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JournalRecord {
    /// Старт сессии + метаданные формата (нужны для починки/чтения сегментов).
    /// Время — мс от эпохи Unix (`u64`: `serde_json` не сериализует `u128`).
    SessionStarted {
        started_at_unix_ms: u64,
        sample_rate_hz: u32,
        channels: u16,
        bit_depth: u16,
        segment_seconds: u32,
    },
    /// Открыт новый сегмент (имя файла фиксируем до записи данных).
    SegmentStarted {
        index: u32,
        path: String,
        started_at_unix_ms: u64,
    },
    /// Сегмент финализирован (известна длина в кадрах).
    SegmentCompleted {
        index: u32,
        path: String,
        frames: u64,
    },
    /// Пауза (с причиной — operator/device_lost).
    Paused { reason: String },
    /// Возобновление после паузы.
    Resumed,
    /// Обрыв/пропажа устройства.
    DeviceLost,
    /// Возврат устройства.
    DeviceBack,
    /// Свободное место упало ниже порога предупреждения.
    DiskLow { free_mb: u64 },
    /// Свободное место достигло критического порога (защитный стоп).
    DiskCritical { free_mb: u64 },
    /// Watchdog перезапустил зависший захват.
    WatchdogRestart,
    /// Достигнут `max_session_hours` — предупреждение (запись продолжается).
    MaxDurationWarning,
    /// Сессия помечена восстановленной после сбоя.
    Recovered,
    /// Корректное завершение сессии.
    Stopped,
    /// Живая разметка (этап 10): закладка/интервал роли — действие оператора
    /// вне аудио-потока. Write-ahead: переживает сбой, реконсилируется в SQLite.
    Annotation(AnnotationRecord),
}

/// Открытый на дозапись журнал сессии.
pub struct Journal {
    file: File,
    path: PathBuf,
}

impl Journal {
    /// Открыть (создав при необходимости) журнал в каталоге сессии на дозапись.
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(JOURNAL_FILE_NAME);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { file, path })
    }

    /// Путь к файлу журнала.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Дописать запись и сбросить её на диск (`fsync`). Журнал — write-ahead:
    /// fsync на каждой записи гарантирует переживание внезапного завершения
    /// процесса (записей немного и они редки относительно аудио-потока).
    pub fn append(&mut self, record: &JournalRecord) -> std::io::Result<()> {
        let mut line = serde_json::to_string(record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.sync_all()?;
        Ok(())
    }
}

/// Результат реплея журнала.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayState {
    /// Метаданные сессии (первая запись `SessionStarted`), если есть.
    pub started: Option<SessionMeta>,
    /// Завершённые сегменты в порядке журнала.
    pub completed_segments: Vec<CompletedSegment>,
    /// Сессия завершена штатно (встречена запись `Stopped`).
    pub stopped: bool,
    /// Сессия уже помечена восстановленной (встречена запись `Recovered`).
    pub recovered: bool,
    /// Действия разметки в порядке журнала (этап 10).
    pub annotations: Vec<AnnotationRecord>,
}

/// Метаданные формата сессии из записи `SessionStarted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    pub started_at_unix_ms: u64,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub bit_depth: u16,
    pub segment_seconds: u32,
}

/// Завершённый сегмент по данным журнала.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedSegment {
    pub index: u32,
    pub path: String,
    pub frames: u64,
}

impl ReplayState {
    /// Незавершённая ли это сессия: была начата, но не остановлена штатно.
    pub fn is_unfinished(&self) -> bool {
        self.started.is_some() && !self.stopped
    }
}

/// Восстановить состояние сессии по журналу. Терпит усечённую/битую **последнюю**
/// строку (частая ситуация при сбое питания во время записи): нечитаемые строки
/// пропускаются, разбор не падает.
pub fn replay(path: &Path) -> std::io::Result<ReplayState> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut state = ReplayState {
        started: None,
        completed_segments: Vec::new(),
        stopped: false,
        recovered: false,
        annotations: Vec::new(),
    };

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // Битую/усечённую строку (например, оборванную сбоем питания) просто
        // пропускаем — это и есть «потеря не более flush-интервала».
        let Ok(record) = serde_json::from_str::<JournalRecord>(&line) else {
            continue;
        };
        match record {
            JournalRecord::SessionStarted {
                started_at_unix_ms,
                sample_rate_hz,
                channels,
                bit_depth,
                segment_seconds,
            } => {
                state.started = Some(SessionMeta {
                    started_at_unix_ms,
                    sample_rate_hz,
                    channels,
                    bit_depth,
                    segment_seconds,
                });
            }
            JournalRecord::SegmentCompleted {
                index,
                path,
                frames,
            } => state.completed_segments.push(CompletedSegment {
                index,
                path,
                frames,
            }),
            JournalRecord::Recovered => state.recovered = true,
            JournalRecord::Stopped => state.stopped = true,
            JournalRecord::Annotation(rec) => state.annotations.push(rec),
            _ => {}
        }
    }

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_record() -> JournalRecord {
        JournalRecord::SessionStarted {
            started_at_unix_ms: 1_700_000_000_000,
            sample_rate_hz: 44_100,
            channels: 1,
            bit_depth: 16,
            segment_seconds: 30,
        }
    }

    #[test]
    fn record_roundtrips_through_json() {
        let rec = meta_record();
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"type\":\"session_started\""));
        let back: JournalRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(rec, back);
    }

    #[test]
    fn append_then_replay_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut j = Journal::open(tmp.path()).unwrap();
        j.append(&meta_record()).unwrap();
        j.append(&JournalRecord::SegmentCompleted {
            index: 1,
            path: "seg-0001.wav".into(),
            frames: 8_000,
        })
        .unwrap();
        j.append(&JournalRecord::SegmentCompleted {
            index: 2,
            path: "seg-0002.wav".into(),
            frames: 4_000,
        })
        .unwrap();
        j.append(&JournalRecord::Stopped).unwrap();

        let state = replay(j.path()).unwrap();
        assert_eq!(state.started.as_ref().unwrap().sample_rate_hz, 44_100);
        assert_eq!(state.completed_segments.len(), 2);
        assert_eq!(state.completed_segments[1].frames, 4_000);
        assert!(state.stopped);
        assert!(!state.is_unfinished());
    }

    #[test]
    fn unfinished_session_detected_without_stop() {
        let tmp = tempfile::tempdir().unwrap();
        let mut j = Journal::open(tmp.path()).unwrap();
        j.append(&meta_record()).unwrap();
        j.append(&JournalRecord::SegmentCompleted {
            index: 1,
            path: "seg-0001.wav".into(),
            frames: 8_000,
        })
        .unwrap();
        // Нет записи Stopped — сессия незавершённая.
        let state = replay(j.path()).unwrap();
        assert!(state.is_unfinished());
        assert!(!state.recovered);
    }

    #[test]
    fn truncated_last_line_is_tolerated() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(JOURNAL_FILE_NAME);
        // Валидная запись + оборванная (усечённая сбоем) последняя строка.
        let good = serde_json::to_string(&meta_record()).unwrap();
        let mut bytes = good.into_bytes();
        bytes.push(b'\n');
        bytes.extend_from_slice(b"{\"type\":\"segment_completed\",\"index\":1,\"pa");
        std::fs::write(&path, bytes).unwrap();

        let state = replay(&path).unwrap();
        // Метаданные прочитаны, оборванная строка пропущена без паники.
        assert!(state.started.is_some());
        assert_eq!(state.completed_segments.len(), 0);
        assert!(state.is_unfinished());
    }

    #[test]
    fn annotation_records_roundtrip_and_replay() {
        use crate::integrity::annotations::{AnnotationAction, AnnotationRecord};
        let tmp = tempfile::tempdir().unwrap();
        let mut j = Journal::open(tmp.path()).unwrap();
        j.append(&meta_record()).unwrap();
        let ann = AnnotationRecord {
            seq: 1,
            action: AnnotationAction::MarkerAdded,
            target_id: "m1".into(),
            category: Some("Инцидент".into()),
            role: None,
            comment: Some("шум в зале".into()),
            offset_samples: 44_100,
            offset_ms: 1_000,
            operator_id: "op-7".into(),
            at_unix_ms: 1_700_000_001_000,
            chain_link: "link1".into(),
        };
        let rec = JournalRecord::Annotation(ann.clone());
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"type\":\"annotation\""));
        assert!(json.contains("\"action\":\"marker_added\""));
        j.append(&rec).unwrap();
        j.append(&JournalRecord::Stopped).unwrap();

        let state = replay(j.path()).unwrap();
        assert_eq!(state.annotations.len(), 1);
        assert_eq!(state.annotations[0], ann);
        assert!(state.stopped);
    }

    #[test]
    fn empty_journal_replays_to_empty_state() {
        let tmp = tempfile::tempdir().unwrap();
        let j = Journal::open(tmp.path()).unwrap();
        let state = replay(j.path()).unwrap();
        assert!(state.started.is_none());
        assert!(!state.is_unfinished());
    }
}
