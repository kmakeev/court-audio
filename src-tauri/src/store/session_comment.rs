//! Свободный комментарий оператора к сессии (этап 10.6, deliverable 6 — мелочи
//! трения).
//!
//! Хранится **файлом в каталоге сессии** (`comment.txt`), а не колонкой манифеста:
//! это локальная заметка оператора, не входит в контур целостности/выгрузки, и
//! файл не требует миграции схемы SQLite и правки многих call-site'ов
//! `SessionRecord`. Пустой/отсутствующий файл → комментария нет.

use std::path::Path;

/// Имя файла комментария в каталоге сессии.
const COMMENT_FILE: &str = "comment.txt";

/// Прочитать комментарий сессии из её каталога (`None` — файла нет/пуст).
pub fn read(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(COMMENT_FILE)).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Записать/очистить комментарий сессии. Пустой текст удаляет файл (нет заметки).
pub fn write(dir: &Path, text: &str) -> std::io::Result<()> {
    let path = dir.join(COMMENT_FILE);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        // Идемпотентно: отсутствие файла — не ошибка.
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    } else {
        std::fs::write(&path, trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read(tmp.path()), None);
        write(tmp.path(), "  свидетель опоздал  ").unwrap();
        assert_eq!(read(tmp.path()).as_deref(), Some("свидетель опоздал"));
    }

    #[test]
    fn empty_text_clears_comment() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "заметка").unwrap();
        assert!(read(tmp.path()).is_some());
        write(tmp.path(), "   ").unwrap();
        assert_eq!(read(tmp.path()), None);
        // Повторная очистка отсутствующего файла не паникует.
        write(tmp.path(), "").unwrap();
    }
}
