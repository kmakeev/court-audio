//! Прожиг экспортного пакета на **data-DVD** (не Audio-CD — метки/целостность
//! в CD-DA не переносятся, см. «Вне объёма» промта) через системные средства:
//! `growisofs` (Astra SE/РЕД ОС/Ubuntu — Linux), IMAPI через PowerShell-скрипт
//! (Windows). На прочих ОС (macOS — станция разработки, оптического привода
//! нет) — заглушка с понятной диагностикой; экспорт в папку остаётся
//! доступен независимо (критерий приёмки промта).
//!
//! **Граница автоматического тестирования.** Построение аргументов команды,
//! поиск утилиты в PATH и верификация хешей по произвольному пути (в тестах —
//! обычный каталог, не примонтированный диск) — юнит-тестируются напрямую.
//! Реальный прожиг на приводе — нет: ни на станции разработки (macOS, без
//! оптического привода), ни в CI такой возможности нет. Проверка прожига на
//! целевых Windows/Linux — ручная, на стенде.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::process::Command;

use super::manifest::CopyFileEntry;
use crate::integrity::hash;

/// Ошибка прожига/верификации DVD.
#[derive(Debug)]
pub enum DvdError {
    /// Привод не найден.
    NoDriveFound,
    /// Утилита прожига не найдена в PATH.
    ToolMissing(String),
    /// Ошибка прожига (код возврата/вывод утилиты).
    BurnFailed(String),
    /// После прожига хеши не совпали (или файл отсутствует).
    VerifyFailed(String),
    /// Ошибка ввода-вывода при чтении для верификации.
    Io(String),
    /// Прожиг не поддержан на этой ОС.
    UnsupportedOs,
}

impl std::fmt::Display for DvdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DvdError::NoDriveFound => write!(f, "оптический привод не найден"),
            DvdError::ToolMissing(t) => write!(f, "утилита прожига «{t}» не найдена"),
            DvdError::BurnFailed(e) => write!(f, "ошибка прожига: {e}"),
            DvdError::VerifyFailed(e) => write!(f, "верификация после прожига не прошла: {e}"),
            DvdError::Io(e) => write!(f, "ошибка ввода-вывода: {e}"),
            DvdError::UnsupportedOs => write!(
                f,
                "прожиг DVD не поддержан на этой ОС; экспорт в папку остаётся доступен"
            ),
        }
    }
}

impl std::error::Error for DvdError {}

/// Найденный привод.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveInfo {
    /// Идентификатор привода для конкретной ОС (напр. `/dev/sr0`, `D:`).
    pub id: String,
    pub label: String,
}

/// Абстракция прожига data-DVD, реализация выбирается компиляцией по целевой
/// ОС (`cfg(target_os)`), не рантаймом.
pub trait DvdBurner {
    /// Найти привод (`None` — привода нет, штатный случай, не ошибка).
    fn detect_drive(&self) -> Result<Option<DriveInfo>, DvdError>;
    /// Прожечь содержимое `source_dir` как data-DVD (Rock Ridge + Joliet) с
    /// меткой тома `label`.
    fn burn(&self, source_dir: &Path, drive: &DriveInfo, label: &str) -> Result<(), DvdError>;
}

/// Метка тома DVD ограничена Joliet-совместимой длиной — не параметр
/// реестра (техническое ограничение формата, не бизнес-настройка).
const VOLUME_LABEL_MAX_LEN: usize = 32;

/// Обрезать метку тома до Joliet-совместимой длины.
pub fn truncate_volume_label(label: &str) -> String {
    label.chars().take(VOLUME_LABEL_MAX_LEN).collect()
}

/// Построить аргументы `growisofs` для прожига `source_dir` как data-DVD на
/// `drive_path` с меткой `label`. Чистая функция — не исполняет процесс,
/// тестируется без привода. Используется реализацией `GrowisofsBurner`
/// (только `cfg(target_os = "linux")`) — на прочих ОС используется лишь в
/// тестах, отсюда `allow(dead_code)` вне Linux/Windows-сборок.
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
fn growisofs_args(source_dir: &Path, drive_path: &str, label: &str) -> Vec<OsString> {
    vec![
        OsString::from("-Z"),
        OsString::from(drive_path),
        OsString::from("-r"),
        OsString::from("-J"),
        OsString::from(format!("-V{}", truncate_volume_label(label))),
        source_dir.as_os_str().to_os_string(),
    ]
}

/// Найти исполняемый файл `name` в `PATH` (без внешней crate-зависимости —
/// эквивалент `which`/`where`). Используется реализациями прожига на
/// Linux/Windows — на прочих ОС используется лишь в тестах.
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
fn find_tool(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(target_os = "windows")]
        {
            let with_exe = dir.join(format!("{name}.exe"));
            if with_exe.is_file() {
                return Some(with_exe);
            }
        }
    }
    None
}

/// Прочитать файлы `root` (примонтированный диск после прожига — в тестах
/// обычный каталог) и сверить их SHA-256 с манифестом пакета.
pub fn verify_package_on_path(root: &Path, files: &[CopyFileEntry]) -> Result<(), DvdError> {
    for entry in files {
        let path = root.join(&entry.name);
        if !path.is_file() {
            return Err(DvdError::VerifyFailed(format!(
                "файл {} отсутствует на носителе",
                entry.name
            )));
        }
        let actual = hash::sha256_file(&path).map_err(|e| DvdError::Io(e.to_string()))?;
        if actual != entry.sha256 {
            return Err(DvdError::VerifyFailed(format!(
                "файл {} — хеш не совпадает с манифестом",
                entry.name
            )));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub struct GrowisofsBurner;

#[cfg(target_os = "linux")]
impl DvdBurner for GrowisofsBurner {
    fn detect_drive(&self) -> Result<Option<DriveInfo>, DvdError> {
        for candidate in ["/dev/sr0", "/dev/sr1", "/dev/dvd"] {
            if Path::new(candidate).exists() {
                return Ok(Some(DriveInfo {
                    id: candidate.to_string(),
                    label: candidate.to_string(),
                }));
            }
        }
        Ok(None)
    }

    fn burn(&self, source_dir: &Path, drive: &DriveInfo, label: &str) -> Result<(), DvdError> {
        let tool = find_tool("growisofs").ok_or_else(|| DvdError::ToolMissing("growisofs".to_string()))?;
        let args = growisofs_args(source_dir, &drive.id, label);
        let output = Command::new(tool)
            .args(&args)
            .output()
            .map_err(|e| DvdError::BurnFailed(e.to_string()))?;
        if !output.status.success() {
            return Err(DvdError::BurnFailed(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub struct ImapiBurner;

/// PowerShell-скрипт прожига через IMAPI2 (без Rust-биндингов к COM —
/// оболочка над системным `powershell.exe`, как и другие ОС-специфичные
/// вызовы в проекте).
#[cfg(target_os = "windows")]
const BURN_SCRIPT: &str = include_str!("burn_windows.ps1");

#[cfg(target_os = "windows")]
impl DvdBurner for ImapiBurner {
    fn detect_drive(&self) -> Result<Option<DriveInfo>, DvdError> {
        let tool = find_tool("powershell.exe")
            .ok_or_else(|| DvdError::ToolMissing("powershell.exe".to_string()))?;
        let output = Command::new(tool)
            .args(["-NoProfile", "-Command", "(New-Object -ComObject IMAPI2.MsftDiscMaster2).Count"])
            .output()
            .map_err(|e| DvdError::Io(e.to_string()))?;
        let count: u32 = String::from_utf8_lossy(&output.stdout).trim().parse().unwrap_or(0);
        if count == 0 {
            return Ok(None);
        }
        Ok(Some(DriveInfo {
            id: "0".to_string(),
            label: "Оптический привод".to_string(),
        }))
    }

    fn burn(&self, source_dir: &Path, drive: &DriveInfo, label: &str) -> Result<(), DvdError> {
        let tool = find_tool("powershell.exe")
            .ok_or_else(|| DvdError::ToolMissing("powershell.exe".to_string()))?;
        let output = Command::new(tool)
            .args([
                "-NoProfile",
                "-Command",
                BURN_SCRIPT,
                "-SourceDir",
                &source_dir.to_string_lossy(),
                "-DriveIndex",
                &drive.id,
                "-VolumeLabel",
                &truncate_volume_label(label),
            ])
            .output()
            .map_err(|e| DvdError::BurnFailed(e.to_string()))?;
        if !output.status.success() {
            return Err(DvdError::BurnFailed(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        Ok(())
    }
}

/// Заглушка на ОС без реализованного прожига (сейчас — macOS, станция
/// разработки; целевые ОС v1/фазы 2 — Windows/Astra/РЕД ОС).
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub struct UnsupportedBurner;

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
impl DvdBurner for UnsupportedBurner {
    fn detect_drive(&self) -> Result<Option<DriveInfo>, DvdError> {
        Err(DvdError::UnsupportedOs)
    }

    fn burn(&self, _source_dir: &Path, _drive: &DriveInfo, _label: &str) -> Result<(), DvdError> {
        Err(DvdError::UnsupportedOs)
    }
}

/// Выбрать реализацию прожига для текущей ОС (компиляция по `cfg`, не
/// рантайм-переключатель).
#[cfg(target_os = "linux")]
pub fn platform_burner() -> impl DvdBurner {
    GrowisofsBurner
}

#[cfg(target_os = "windows")]
pub fn platform_burner() -> impl DvdBurner {
    ImapiBurner
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn platform_burner() -> impl DvdBurner {
    UnsupportedBurner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growisofs_args_builds_expected_command_line() {
        let args = growisofs_args(Path::new("/tmp/pkg"), "/dev/sr0", "Заседание 1-123");
        let joined: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            joined,
            vec![
                "-Z".to_string(),
                "/dev/sr0".to_string(),
                "-r".to_string(),
                "-J".to_string(),
                "-VЗаседание 1-123".to_string(),
                "/tmp/pkg".to_string(),
            ]
        );
    }

    #[test]
    fn truncate_volume_label_respects_joliet_limit() {
        let long = "А".repeat(50);
        let truncated = truncate_volume_label(&long);
        assert_eq!(truncated.chars().count(), VOLUME_LABEL_MAX_LEN);
    }

    #[test]
    fn find_tool_returns_none_when_absent_from_path() {
        // Изолируем PATH для этого теста, чтобы не зависеть от реального
        // окружения (и не конфликтовать с параллельными тестами, меняющими
        // тот же env var — see cargo test --test-threads=1 caveat below).
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("PATH");
        std::env::set_var("PATH", tmp.path());
        let found = find_tool("definitely-not-a-real-tool-name-xyz");
        if let Some(p) = prev {
            std::env::set_var("PATH", p);
        }
        assert!(found.is_none());
    }

    #[test]
    fn find_tool_finds_executable_placed_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        let tool_path = tmp.path().join("my-burn-tool");
        std::fs::write(&tool_path, b"#!/bin/sh\n").unwrap();
        let prev = std::env::var_os("PATH");
        std::env::set_var("PATH", tmp.path());
        let found = find_tool("my-burn-tool");
        if let Some(p) = prev {
            std::env::set_var("PATH", p);
        }
        assert_eq!(found, Some(tool_path));
    }

    fn file_entry(name: &str, content: &[u8]) -> CopyFileEntry {
        CopyFileEntry {
            name: name.to_string(),
            sha256: hash::sha256_bytes(content),
            size_bytes: content.len() as u64,
            track_id: None,
            role: None,
        }
    }

    #[test]
    fn verify_package_on_path_ok_when_hashes_match() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.wav"), b"audio-bytes-a").unwrap();
        let files = vec![file_entry("a.wav", b"audio-bytes-a")];
        assert!(verify_package_on_path(tmp.path(), &files).is_ok());
    }

    #[test]
    fn verify_package_on_path_fails_on_tampered_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.wav"), b"tampered-bytes").unwrap();
        let files = vec![file_entry("a.wav", b"audio-bytes-a")];
        let err = verify_package_on_path(tmp.path(), &files).unwrap_err();
        assert!(matches!(err, DvdError::VerifyFailed(_)));
    }

    #[test]
    fn verify_package_on_path_fails_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let files = vec![file_entry("missing.wav", b"whatever")];
        let err = verify_package_on_path(tmp.path(), &files).unwrap_err();
        assert!(matches!(err, DvdError::VerifyFailed(_)));
    }
}
