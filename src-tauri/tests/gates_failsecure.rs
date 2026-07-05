//! Безопасность гейтов и ключа станции (этап 13.5 — `promts/13_5_gates_failsecure.md`,
//! R-003 + R-004). Всё портируемо в CI (без Tauri/устройства/сети):
//!
//! - **R-003 fail-secure:** повреждённый `settings.json` НЕ размыкает гейты —
//!   разбор даёт ошибку (не тихие дефолты), гейт старта и админ-гейт смыкаются;
//! - **R-004 ключ станции:** без ключа зашифрованный блоб (кэш офлайн-сессии,
//!   админ-PIN) **не пишется** и сбой НЕ проглатывается (явная ошибка), а с
//!   корректным ключом офлайн-вход по PIN и админ-PIN работают (регресс).

use std::sync::Mutex;

use court_audio_lib::ipc::admin_cmds::admin_change_denied;
use court_audio_lib::ipc::audio_cmds::segment_encryption_key;
use court_audio_lib::ipc::auth_cmds::start_gate_decision;
use court_audio_lib::ipc::{parse_settings_failsecure, CONFIG_CORRUPT_MESSAGE};
use court_audio_lib::settings::{KeySource, Settings};
use court_audio_lib::store::crypto::{self, ensure_station_key};
use court_audio_lib::store::{admin_pin, auth_cache};
use court_audio_lib::sync::auth::{hash_pin, CachedSession};

// `COURT_AUDIO_STATION_PASSPHRASE` — process-global. Env-чувствительная проверка
// одна и держит весь жизненный цикл переменной внутри себя под мьютексом (как в
// unit-тестах admin_pin), чтобы параллельные тесты не гоняли значение.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ── R-003: fail-secure при повреждённом settings.json ─────────────────────────

#[test]
fn corrupt_settings_parse_is_failsecure() {
    // Валидный JSON разбирается штатно.
    let ok = serde_json::to_string(&Settings::default()).unwrap();
    assert!(parse_settings_failsecure(&ok).is_ok());
    // Битый JSON → ошибка с понятной админу диагностикой, а НЕ тихие дефолты.
    let err = parse_settings_failsecure("{ это не настройки ").unwrap_err();
    assert!(err.contains(CONFIG_CORRUPT_MESSAGE), "сообщение: {err}");
}

#[test]
fn corrupt_config_closes_start_gate() {
    // Повреждённый конфиг НЕ пускает старт даже при вошедшем операторе (fail-secure).
    assert!(start_gate_decision(Err("повреждено"), true).is_err());
    // Корректный конфиг: политика `required_to_start` (дефолт true) работает штатно.
    let s = Settings::default();
    assert!(start_gate_decision(Ok(&s), false).is_err()); // без оператора — закрыто
    assert!(start_gate_decision(Ok(&s), true).is_ok()); // с оператором — открыто
}

#[test]
fn corrupt_config_denies_admin_change() {
    let s = Settings::default(); // admin.pin.required = true
    // Корректный конфиг: админ-изменение без разблокировки — отказ; с — разрешено.
    assert!(admin_change_denied(Ok(&s), true, false));
    assert!(!admin_change_denied(Ok(&s), true, true));
    // Чисто оператор-изменение не блокируется.
    assert!(!admin_change_denied(Ok(&s), false, false));
    // Повреждённый конфиг: любое изменение отклоняется — обойти админ-PIN нельзя.
    assert!(admin_change_denied(Err("повреждено"), false, true));
}

// ── R-004: обязательный ключ станции; сбой шифрования не проглатывается ────────

#[test]
fn station_key_mandatory_and_failure_not_swallowed() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // ── Нет ключа станции: валидация падает, блоб НЕ пишется, сбой виден ──
    std::env::remove_var(crypto::PASSPHRASE_ENV);
    assert!(ensure_station_key(KeySource::Passphrase, root).is_err());

    let session = sample_session();
    // Кэш офлайн-сессии не сохраняется и возвращает ошибку (а не «успех»).
    assert!(auth_cache::save(root, KeySource::Passphrase, &session).is_err());
    assert!(auth_cache::load(root, KeySource::Passphrase).unwrap().is_none());
    // Админ-PIN не провижинится без ключа станции.
    assert!(admin_pin::provision(root, KeySource::Passphrase, "2468").is_err());
    assert!(!admin_pin::is_provisioned(root));

    // ── С корректным ключом: офлайн-вход по PIN и админ-PIN работают (регресс) ──
    std::env::set_var(crypto::PASSPHRASE_ENV, "station-secret-13-5");
    assert!(ensure_station_key(KeySource::Passphrase, root).is_ok());

    auth_cache::save(root, KeySource::Passphrase, &session).unwrap();
    let back = auth_cache::load(root, KeySource::Passphrase).unwrap().unwrap();
    assert_eq!(back, session);

    admin_pin::provision(root, KeySource::Passphrase, "2468").unwrap();
    assert!(admin_pin::verify(root, KeySource::Passphrase, "2468").unwrap());
    assert!(!admin_pin::verify(root, KeySource::Passphrase, "0000").unwrap());

    std::env::remove_var(crypto::PASSPHRASE_ENV);
}

// ── R-013 (этап 13.7): fail-secure гейт шифрования сегментов ──────────────────

#[test]
fn encrypt_at_rest_without_key_blocks_recording_start() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut settings = Settings::default(); // storage.encrypt_at_rest = true

    // Нет ключа станции: старт записи блокируется громкой ошибкой —
    // никакого тихого plaintext-фолбэка.
    std::env::remove_var(crypto::PASSPHRASE_ENV);
    let err = segment_encryption_key(&settings, root).unwrap_err();
    assert!(err.contains("ключ станции"), "диагностика понятна: {err}");

    // С ключом — гейт открыт, ключ разрешён для writer-потока.
    std::env::set_var(crypto::PASSPHRASE_ENV, "station-secret-13-7");
    assert!(segment_encryption_key(&settings, root).unwrap().is_some());
    std::env::remove_var(crypto::PASSPHRASE_ENV);

    // Шифрование выключено: ключ не нужен, поведение прежнее (plaintext WAV).
    settings.storage.encrypt_at_rest = false;
    assert!(segment_encryption_key(&settings, root).unwrap().is_none());
}

fn sample_session() -> CachedSession {
    let (pin_salt, pin_hash) = hash_pin("2468");
    CachedSession {
        operator_id: "42".into(),
        full_name: "Иванов И. И.".into(),
        role: "assistant".into(),
        refresh_token: "refresh-xyz".into(),
        obtained_at_unix_ms: 1_700_000_000_000,
        pin_salt,
        pin_hash,
    }
}
