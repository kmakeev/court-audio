//! SHA-256 сегментов и хеш-цепочка (этап 03 — `promts/03_store_integrity.md`,
//! deliverable 2).
//!
//! Хеш считается по **каноничному аудио-контенту** (байты WAV-файла **до**
//! шифрования at-rest): сервер при выгрузке (`06`/`07`) верифицирует ту же
//! последовательность байт, что получит. Шифрование — отдельный локальный слой,
//! в хеш не входит.
//!
//! Хеш-цепочка связывает сегменты: звено `link[i] = H(link[i-1] || hash[i])`,
//! genesis (`i = 0`) — `H(hash[0])`. Финальное звено сессии фиксируется в
//! манифесте; изменение любого сегмента ломает все последующие звенья
//! (tamper detection). Алгоритм — `Settings.integrity.segment_hash` (`sha256`),
//! цепочка — под флагом `Settings.integrity.hash_chain`.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Ошибка вычисления хеша/цепочки.
#[derive(Debug)]
pub enum HashError {
    /// Ошибка ввода-вывода при чтении сегмента.
    Io(String),
    /// Запрошен неподдерживаемый алгоритм (поддержан только `sha256`).
    UnsupportedAlgorithm(String),
}

impl std::fmt::Display for HashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HashError::Io(e) => write!(f, "ошибка ввода-вывода при хешировании: {e}"),
            HashError::UnsupportedAlgorithm(a) => {
                write!(f, "неподдерживаемый алгоритм хеша: {a} (ожидался sha256)")
            }
        }
    }
}

impl std::error::Error for HashError {}

/// Единственный поддерживаемый в v1 алгоритм (`configuration.md` →
/// `integrity.segment_hash`). ГОСТ-хеш — фаза 2.
pub const ALGORITHM_SHA256: &str = "sha256";

/// Проверить, что настроенный алгоритм поддержан (вызывать при инициализации,
/// чтобы не зашивать имя алгоритма в логику и ловить опечатки в конфиге).
pub fn ensure_supported(algorithm: &str) -> Result<(), HashError> {
    if algorithm == ALGORITHM_SHA256 {
        Ok(())
    } else {
        Err(HashError::UnsupportedAlgorithm(algorithm.to_string()))
    }
}

/// Hex-представление байт в нижнем регистре (без внешней зависимости).
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

/// SHA-256 произвольного буфера (hex).
pub fn sha256_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    to_hex(&hasher.finalize())
}

/// SHA-256 файла (hex), потоковым чтением — сегмент может быть крупным.
pub fn sha256_file(path: &Path) -> Result<String, HashError> {
    let mut file = std::fs::File::open(path).map_err(|e| HashError::Io(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| HashError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Звено цепочки: `H(prev_link || segment_hash)`; для genesis (`prev = None`) —
/// `H(segment_hash)`. Работаем с hex-строками, чтобы цепочка была воспроизводима
/// и на сервере (`07`).
pub fn chain_link(prev_link: Option<&str>, segment_hash: &str) -> String {
    let mut hasher = Sha256::new();
    if let Some(prev) = prev_link {
        hasher.update(prev.as_bytes());
    }
    hasher.update(segment_hash.as_bytes());
    to_hex(&hasher.finalize())
}

/// Построить полную цепочку звеньев по упорядоченным хешам сегментов.
/// `chain[i]` соответствует сегменту `i`; финальное звено — `chain.last()`.
pub fn build_chain(segment_hashes: &[String]) -> Vec<String> {
    let mut chain = Vec::with_capacity(segment_hashes.len());
    let mut prev: Option<String> = None;
    for hash in segment_hashes {
        let link = chain_link(prev.as_deref(), hash);
        prev = Some(link.clone());
        chain.push(link);
    }
    chain
}

/// Финальное звено цепочки (итог сессии) по хешам сегментов, или `None` для
/// пустой сессии.
pub fn final_link(segment_hashes: &[String]) -> Option<String> {
    build_chain(segment_hashes).pop()
}

/// Верифицировать целостность: пересчитать цепочку по предъявленным хешам и
/// сверить с сохранёнными звеньями и финалом. Любое расхождение (подмена
/// сегмента → другой хеш → другие звенья) даёт `false`.
pub fn verify_chain(
    segment_hashes: &[String],
    stored_links: &[String],
    expected_final: Option<&str>,
) -> bool {
    if segment_hashes.len() != stored_links.len() {
        return false;
    }
    let recomputed = build_chain(segment_hashes);
    if recomputed != stored_links {
        return false;
    }
    match (recomputed.last().map(|s| s.as_str()), expected_final) {
        (Some(last), Some(exp)) => last == exp,
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        // SHA-256("") — известный вектор.
        assert_eq!(
            sha256_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_file_equals_buffer() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("seg.bin");
        let data = b"court-audio segment bytes";
        std::fs::write(&path, data).unwrap();
        assert_eq!(sha256_file(&path).unwrap(), sha256_bytes(data));
    }

    #[test]
    fn genesis_link_is_hash_of_first() {
        let h0 = sha256_bytes(b"seg0");
        assert_eq!(chain_link(None, &h0), sha256_bytes(h0.as_bytes()));
    }

    #[test]
    fn chain_links_each_to_previous() {
        let hashes = vec![
            sha256_bytes(b"seg0"),
            sha256_bytes(b"seg1"),
            sha256_bytes(b"seg2"),
        ];
        let chain = build_chain(&hashes);
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0], chain_link(None, &hashes[0]));
        assert_eq!(chain[1], chain_link(Some(&chain[0]), &hashes[1]));
        assert_eq!(chain[2], chain_link(Some(&chain[1]), &hashes[2]));
        assert_eq!(final_link(&hashes).as_deref(), Some(chain[2].as_str()));
    }

    #[test]
    fn verify_accepts_intact_chain() {
        let hashes = vec![sha256_bytes(b"a"), sha256_bytes(b"b")];
        let chain = build_chain(&hashes);
        let fin = chain.last().cloned();
        assert!(verify_chain(&hashes, &chain, fin.as_deref()));
    }

    #[test]
    fn tamper_breaks_verification() {
        let hashes = vec![sha256_bytes(b"a"), sha256_bytes(b"b"), sha256_bytes(b"c")];
        let chain = build_chain(&hashes);
        let fin = chain.last().cloned();

        // Подмена контента второго сегмента → другой хеш.
        let mut tampered = hashes.clone();
        tampered[1] = sha256_bytes(b"b-tampered");
        // Хеши не совпадают со звеньями — верификация падает.
        assert!(!verify_chain(&tampered, &chain, fin.as_deref()));

        // Даже если злоумышленник пересчитает финал, сохранённые звенья (из
        // манифеста/сервера) не совпадут с пересчётом.
        let recomputed = build_chain(&tampered);
        assert_ne!(recomputed, chain);
    }

    #[test]
    fn ensure_supported_rejects_unknown() {
        assert!(ensure_supported("sha256").is_ok());
        assert!(matches!(
            ensure_supported("gost"),
            Err(HashError::UnsupportedAlgorithm(_))
        ));
    }
}
