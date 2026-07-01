//! Инвариант-проверка манифеста упаковки (этап 08 — `promts/08_packaging.md`).
//!
//! Критерии приёмки этапа (оффлайн-установка, подпись, без авто-апдейта)
//! проверяются установкой на чистых ОС и не автоматизируются на dev-машине.
//! Здесь — быстрый регрессионный тест `tauri.conf.json`: ловит случайное
//! ослабление упаковочных гарантий (пропажу системных зависимостей, откат
//! WebView2 на онлайн-бутстраппер, включение интернет-апдейтера, потерю
//! метаданных правообладателя). Значения — build-метаданные, живут в
//! `tauri.conf.json` (см. docs/packaging.md), а не в реестре настроек.

use serde_json::Value;

fn config() -> Value {
    let raw = include_str!("../tauri.conf.json");
    serde_json::from_str(raw).expect("tauri.conf.json — валидный JSON")
}

fn str_array(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("ожидался массив")
        .iter()
        .map(|s| s.as_str().expect("ожидалась строка").to_string())
        .collect()
}

#[test]
fn targets_explicit_and_cover_matrix() {
    let cfg = config();
    let targets = str_array(&cfg["bundle"]["targets"]);
    // Явный список вместо непрозрачного "all" — прозрачность матрицы.
    for expected in ["deb", "rpm", "nsis", "dmg"] {
        assert!(
            targets.iter().any(|t| t == expected),
            "в bundle.targets нет таргета {expected}: {targets:?}"
        );
    }
}

#[test]
fn linux_deb_declares_webkit_and_audio() {
    let cfg = config();
    let depends = str_array(&cfg["bundle"]["linux"]["deb"]["depends"]);
    assert!(
        depends.iter().any(|d| d.contains("webkit2gtk")),
        "deb.depends без webkit2gtk (оффлайн-установка Astra): {depends:?}"
    );
    assert!(
        depends.iter().any(|d| d.contains("asound")),
        "deb.depends без ALSA (libasound2): {depends:?}"
    );
}

#[test]
fn linux_rpm_declares_webkit_and_audio() {
    let cfg = config();
    let depends = str_array(&cfg["bundle"]["linux"]["rpm"]["depends"]);
    assert!(!depends.is_empty(), "rpm.depends пуст (РЕД ОС)");
    assert!(
        depends.iter().any(|d| d.contains("webkit2gtk")),
        "rpm.depends без webkit2gtk: {depends:?}"
    );
    assert!(
        depends.iter().any(|d| d.contains("alsa")),
        "rpm.depends без ALSA (alsa-lib): {depends:?}"
    );
}

#[test]
fn windows_webview2_is_offline_fixed_runtime() {
    let cfg = config();
    let mode = &cfg["bundle"]["windows"]["webviewInstallMode"]["type"];
    // Оффлайн-установка в закрытом контуре: фикс-версия рантайма в бандле,
    // НЕ онлайн-бутстраппер, который тянет рантайм из сети.
    assert_eq!(
        mode.as_str(),
        Some("fixedRuntime"),
        "webviewInstallMode должен быть fixedRuntime (оффлайн), а не {mode}"
    );
}

#[test]
fn publisher_and_copyright_present() {
    let cfg = config();
    for key in ["publisher", "copyright"] {
        let v = cfg["bundle"][key].as_str().unwrap_or("");
        assert!(
            !v.trim().is_empty(),
            "bundle.{key} пуст (метаданные Реестра)"
        );
    }
}

#[test]
fn no_internet_updater_configured() {
    let cfg = config();
    // Решение: ручные подписанные пакеты, без публичного авто-апдейта.
    // Плагин updater не подключаем → секции plugins.updater быть не должно.
    let updater = &cfg["plugins"]["updater"];
    assert!(
        updater.is_null(),
        "обнаружен plugins.updater — авто-апдейт вне объёма v1 (этап 08): {updater}"
    );
}
