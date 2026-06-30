//! Дублирующая дорожка (опц.) — этап 02 (`promts/02_recorder_reliability.md`,
//! deliverable 7).
//!
//! При `reliability.mirror.enabled=true` завершённые сегменты дополнительно
//! копируются на второй носитель (`reliability.mirror.path`). Зеркалирование —
//! **best-effort**: сбой зеркала логируется и **не влияет** на основную запись
//! (по решению заказчика самопроизвольного переключения записи на зеркало нет).

use std::path::{Path, PathBuf};

use crate::recorder::segment_writer::SegmentInfo;

/// Зеркало сегментов на второй носитель.
pub struct Mirror {
    dir: PathBuf,
}

impl Mirror {
    /// Создать зеркало в каталоге `dir` (создаётся при необходимости). Возвращает
    /// `Err`, если каталог нельзя подготовить — вызывающий решает, как реагировать
    /// (на старте записи это лишь предупреждение, основная запись продолжается).
    pub fn new(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    /// Скопировать завершённый сегмент на зеркало. Best-effort: возвращаемую
    /// ошибку вызывающий только логирует, основная дорожка не страдает.
    pub fn mirror_segment(&self, segment: &SegmentInfo) -> std::io::Result<u64> {
        let file_name = segment.path.file_name().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "нет имени файла")
        })?;
        let dst = self.dir.join(file_name);
        std::fs::copy(&segment.path, &dst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::recorder::segment_writer::{SegmentConfig, SegmentWriter};

    fn make_segment(dir: &Path) -> SegmentInfo {
        let cfg = SegmentConfig {
            dir: dir.to_path_buf(),
            sample_rate_hz: 8_000,
            channels: 1,
            bits_per_sample: 16,
            segment_seconds: 3600,
            flush_interval: Duration::from_millis(1_500),
        };
        let mut w = SegmentWriter::new(cfg).unwrap();
        w.write_samples(&(0..100).map(|i| i as i16).collect::<Vec<_>>())
            .unwrap();
        w.finalize().unwrap().remove(0)
    }

    #[test]
    fn mirrors_segment_byte_for_byte() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("primary");
        std::fs::create_dir_all(&src_dir).unwrap();
        let seg = make_segment(&src_dir);

        let mirror_dir = tmp.path().join("mirror");
        let mirror = Mirror::new(&mirror_dir).unwrap();
        mirror.mirror_segment(&seg).unwrap();

        let copied = mirror_dir.join(seg.path.file_name().unwrap());
        assert!(copied.exists());
        assert_eq!(
            std::fs::read(&seg.path).unwrap(),
            std::fs::read(&copied).unwrap()
        );
    }

    #[test]
    fn mirror_failure_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("primary");
        std::fs::create_dir_all(&src_dir).unwrap();
        let seg = make_segment(&src_dir);

        let mirror = Mirror::new(&tmp.path().join("mirror")).unwrap();
        // Удаляем исходный сегмент -> copy вернёт ошибку, но без паники.
        std::fs::remove_file(&seg.path).unwrap();
        assert!(mirror.mirror_segment(&seg).is_err());
    }
}
