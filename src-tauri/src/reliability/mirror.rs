//! Дублирующая дорожка (опц.) — этап 02 (`promts/02_recorder_reliability.md`,
//! deliverable 7); структура зеркала — этап 13.3
//! (`promts/13_3_mirror_structure.md`, R-010).
//!
//! При `reliability.mirror.enabled=true` завершённые сегменты **и метаданные**
//! (журнал(ы), `tracks.json`) дополнительно копируются на второй носитель
//! (`reliability.mirror.path`), **повторяя дерево основного места**:
//! `<mirror>/<session>/<track>/…`. Так зеркало самодостаточно — по нему
//! реконсилируется сессия ([`crate::store::reconcile`]) и выгружается в
//! `ex_system`, без коллизий имён (индекс сегмента с 1 **на дорожку** → в плоском
//! каталоге имена совпадали и `std::fs::copy` тихо перезаписывал файл).
//!
//! Зеркалирование — **best-effort**: сбой зеркала логируется и **не влияет** на
//! основную запись (по решению заказчика самопроизвольного переключения записи
//! на зеркало нет). Файлы копируются **как есть** (байт-в-байт): шифрование
//! at-rest наследуется от основного места (когда R-004/этап 13.5 подключит
//! `store::crypto` в live-путь, на зеркало поедет тот же `.enc`-блоб) — зеркало
//! своего криптопути не имеет и plaintext поверх шифрованного не пишет.

use std::path::{Path, PathBuf};

use crate::recorder::segment_writer::SegmentInfo;

/// Зеркало сегментов/метаданных на второй носитель с сохранением структуры.
///
/// `storage_root` — корень основного хранилища (`storage.root_path`): по нему
/// вычисляется путь файла **относительно** основного места, который затем
/// воспроизводится под `mirror_root`. Так один и тот же код обслуживает
/// одноканальную (`<session>/seg-*.wav`) и многоканальную
/// (`<session>/<track>/seg-*.wav`) раскладку без явной передачи имён.
pub struct Mirror {
    storage_root: PathBuf,
    mirror_root: PathBuf,
}

impl Mirror {
    /// Создать зеркало: `mirror_root` (каталог второго носителя) создаётся при
    /// необходимости. Возвращает `Err`, если каталог нельзя подготовить —
    /// вызывающий решает, как реагировать (на старте записи это лишь
    /// предупреждение, основная запись продолжается).
    pub fn new(storage_root: &Path, mirror_root: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(mirror_root)?;
        Ok(Self {
            storage_root: storage_root.to_path_buf(),
            mirror_root: mirror_root.to_path_buf(),
        })
    }

    /// Путь-двойник `src` под `mirror_root`, сохраняя структуру относительно
    /// `storage_root`. `Err`, если `src` не лежит под корнем хранилища (тогда
    /// однозначно реконструировать дерево нельзя — best-effort пропускает файл).
    fn dst_for(&self, src: &Path) -> std::io::Result<PathBuf> {
        let rel = src.strip_prefix(&self.storage_root).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{src:?} вне корня хранилища {:?}", self.storage_root),
            )
        })?;
        Ok(self.mirror_root.join(rel))
    }

    /// Скопировать произвольный файл основного места на зеркало, воспроизведя его
    /// относительный путь (создаёт подкаталоги). Best-effort: возвращаемую ошибку
    /// вызывающий только логирует.
    pub fn mirror_file(&self, src: &Path) -> std::io::Result<u64> {
        let dst = self.dst_for(src)?;
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, &dst)
    }

    /// Скопировать завершённый сегмент на зеркало в его подкаталог сессии/дорожки.
    pub fn mirror_segment(&self, segment: &SegmentInfo) -> std::io::Result<u64> {
        self.mirror_file(&segment.path)
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
    fn mirrors_segment_preserving_session_track_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let storage_root = tmp.path().join("recordings");
        let track_dir = storage_root.join("session-1").join("track-00-judge");
        std::fs::create_dir_all(&track_dir).unwrap();
        let seg = make_segment(&track_dir);

        let mirror_root = tmp.path().join("mirror");
        let mirror = Mirror::new(&storage_root, &mirror_root).unwrap();
        mirror.mirror_segment(&seg).unwrap();

        // Зеркало повторяет структуру: <mirror>/session-1/track-00-judge/seg-*.wav.
        let rel = seg.path.strip_prefix(&storage_root).unwrap();
        let copied = mirror_root.join(rel);
        assert!(copied.exists());
        assert!(copied.to_string_lossy().contains("session-1/track-00-judge"));
        assert_eq!(
            std::fs::read(&seg.path).unwrap(),
            std::fs::read(&copied).unwrap()
        );
    }

    #[test]
    fn no_collision_between_tracks_with_same_segment_name() {
        // Ядро R-010: индекс сегмента с 1 **на дорожку** → в плоском каталоге
        // seg-0001 двух дорожек совпадали и copy тихо перезаписывал. Со структурой
        // они попадают в разные подкаталоги — оба файла целы.
        let tmp = tempfile::tempdir().unwrap();
        let storage_root = tmp.path().join("recordings");
        let mirror_root = tmp.path().join("mirror");
        let mirror = Mirror::new(&storage_root, &mirror_root).unwrap();

        let a = storage_root
            .join("session-1")
            .join("track-00-judge")
            .join("seg-0001-100.wav");
        let b = storage_root
            .join("session-1")
            .join("track-01-defense")
            .join("seg-0001-100.wav"); // то же имя файла, другая дорожка
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::create_dir_all(b.parent().unwrap()).unwrap();
        std::fs::write(&a, b"judge-audio").unwrap();
        std::fs::write(&b, b"defense-audio").unwrap();

        mirror.mirror_file(&a).unwrap();
        mirror.mirror_file(&b).unwrap();

        let ma = mirror_root.join(a.strip_prefix(&storage_root).unwrap());
        let mb = mirror_root.join(b.strip_prefix(&storage_root).unwrap());
        assert_eq!(std::fs::read(&ma).unwrap(), b"judge-audio");
        assert_eq!(std::fs::read(&mb).unwrap(), b"defense-audio", "нет перезаписи");
    }

    #[test]
    fn mirrors_metadata_file_into_session_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let storage_root = tmp.path().join("recordings");
        let session = storage_root.join("session-1");
        std::fs::create_dir_all(&session).unwrap();
        let tracks_json = session.join("tracks.json");
        std::fs::write(&tracks_json, b"{\"tracks\":[]}").unwrap();

        let mirror_root = tmp.path().join("mirror");
        let mirror = Mirror::new(&storage_root, &mirror_root).unwrap();
        mirror.mirror_file(&tracks_json).unwrap();

        let copied = mirror_root.join("session-1").join("tracks.json");
        assert_eq!(std::fs::read(&copied).unwrap(), b"{\"tracks\":[]}");
    }

    #[test]
    fn mirror_failure_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let storage_root = tmp.path().join("recordings");
        let session = storage_root.join("session-1");
        std::fs::create_dir_all(&session).unwrap();
        let seg = make_segment(&session);

        let mirror = Mirror::new(&storage_root, &tmp.path().join("mirror")).unwrap();
        // Удаляем исходный сегмент -> copy вернёт ошибку, но без паники.
        std::fs::remove_file(&seg.path).unwrap();
        assert!(mirror.mirror_segment(&seg).is_err());
    }

    #[test]
    fn file_outside_storage_root_is_error_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let mirror = Mirror::new(&tmp.path().join("recordings"), &tmp.path().join("mirror"))
            .unwrap();
        let stray = tmp.path().join("elsewhere.wav");
        std::fs::write(&stray, b"x").unwrap();
        assert!(mirror.mirror_file(&stray).is_err());
    }
}
