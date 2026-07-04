//! Проверка перед заседанием — self-test одной кнопкой (этап 10.6,
//! `promts/10_6_ux_polish.md`, deliverable 1).
//!
//! Агрегирует уже существующую диагностику ядра в однозначный чек-лист «можно
//! начинать / вот что исправить»: устройство отвечает, места на диске достаточно
//! (пороги `reliability.*`), сервер доступен, вход оператора выполнен (10.3),
//! незавершённых сессий нет. **Новой бизнес-логики нет** — только чтение
//! состояния (`audio::devices`, `reliability::disk_monitor`, `AuthState`,
//! `recorder::recovery`, реестр).
//!
//! Классификация вынесена в чистую [`build_report`] (единственная тестируемая
//! точка, по образцу `ui_cmds::tray_tooltip` / `disk_monitor::classify`); команда
//! лишь наполняет входы реальным состоянием.

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::audio::devices::list_input_devices;
use crate::ipc::auth_cmds::AuthState;
use crate::ipc::{load_settings, resolve_storage_root};
use crate::recorder::recovery;
use crate::reliability::disk_monitor::{classify, free_space_mb, DiskStatus, DiskThresholds};

/// Таймаут best-effort проверки доступности сервера (мс). Не бизнес-параметр
/// реестра: техническая граница, чтобы self-test не «висел» на мёртвом адресе —
/// сервер недоступен трактуется как предупреждение (оффлайн-старт допустим).
const SERVER_PROBE_TIMEOUT_MS: u64 = 2_500;

/// Статус одной проверки. `Warn` — не блокирует старт (оффлайн-сервер, мало
/// места), но выводится оператору; `Fail` — блокирует «можно начинать».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

/// Одна позиция чек-листа. `fix` — маш.-читаемое действие для кнопки «Исправить»
/// (UI мапит на навигацию: `open_settings`/`open_record`/`open_login`/…).
#[derive(Debug, Clone, Serialize)]
pub struct SelfTestCheck {
    pub id: &'static str,
    pub label: &'static str,
    pub status: CheckStatus,
    pub detail: String,
    /// Действие исправления (None — исправлять нечего).
    pub fix: Option<&'static str>,
}

/// Итог self-test: чек-лист + агрегат «можно начинать» (нет ни одного `Fail`).
#[derive(Debug, Clone, Serialize)]
pub struct SelfTestReport {
    pub checks: Vec<SelfTestCheck>,
    pub ready: bool,
}

/// Входы для чистой классификации (собираются командой из реального состояния).
#[derive(Debug, Clone)]
pub struct SelfTestInputs {
    /// Сколько устройств ввода нашлось.
    pub device_count: usize,
    /// Выбранное в настройках устройство присутствует среди найденных (или
    /// выбрано «системное по умолчанию» — тогда достаточно наличия любого).
    pub selected_present: bool,
    /// Свободное место на томе хранилища (МиБ).
    pub free_mb: u64,
    pub disk: DiskThresholds,
    /// Задан ли `sync.server_base_url`.
    pub server_configured: bool,
    /// Доступность сервера: `Some(true/false)` после проверки, `None` — не
    /// проверяли (сервер не настроен).
    pub server_reachable: Option<bool>,
    /// Требуется ли вход оператора перед стартом (`auth.operator.required_to_start`).
    pub operator_required: bool,
    /// Вошёл ли оператор (активная сессия в памяти ядра).
    pub operator_present: bool,
    /// Число незавершённых (recoverable) сессий.
    pub unfinished_count: usize,
    /// Выводится ли ключ станции (`storage.key_source`) — R-004, этап 13.5.
    pub station_key_available: bool,
    /// Включено ли шифрование ПДн at-rest (`storage.encrypt_at_rest`).
    pub encrypt_at_rest: bool,
}

/// Собрать чек-лист по входам. Чистая функция — вся логика статусов здесь.
pub fn build_report(inputs: &SelfTestInputs) -> SelfTestReport {
    let mut checks = Vec::with_capacity(6);

    // 1. Устройство ввода отвечает и выбранное присутствует.
    checks.push(if inputs.device_count == 0 {
        SelfTestCheck {
            id: "device",
            label: "Устройство ввода",
            status: CheckStatus::Fail,
            detail: "Микрофон не найден. Подключите устройство записи.".to_string(),
            fix: Some("open_record"),
        }
    } else if !inputs.selected_present {
        SelfTestCheck {
            id: "device",
            label: "Устройство ввода",
            status: CheckStatus::Warn,
            detail: "Выбранное в настройках устройство недоступно — будет использовано другое."
                .to_string(),
            fix: Some("open_settings"),
        }
    } else {
        SelfTestCheck {
            id: "device",
            label: "Устройство ввода",
            status: CheckStatus::Ok,
            detail: format!("Найдено устройств: {}.", inputs.device_count),
            fix: None,
        }
    });

    // 2. Свободное место (пороги reliability.*).
    checks.push(match classify(inputs.free_mb, inputs.disk) {
        DiskStatus::Ok => SelfTestCheck {
            id: "disk",
            label: "Свободное место на диске",
            status: CheckStatus::Ok,
            detail: format!("Свободно {} МБ.", inputs.free_mb),
            fix: None,
        },
        DiskStatus::Low => SelfTestCheck {
            id: "disk",
            label: "Свободное место на диске",
            status: CheckStatus::Warn,
            detail: format!(
                "Свободно {} МБ — ниже порога предупреждения ({} МБ). Освободите место.",
                inputs.free_mb, inputs.disk.low_mb
            ),
            fix: None,
        },
        DiskStatus::Critical => SelfTestCheck {
            id: "disk",
            label: "Свободное место на диске",
            status: CheckStatus::Fail,
            detail: format!(
                "Свободно {} МБ — критически мало ({} МБ). Записи не хватит места.",
                inputs.free_mb, inputs.disk.critical_mb
            ),
            fix: None,
        },
    });

    // 3. Сервер доступен. Оффлайн-старт допустим по дизайну → недоступность —
    //    предупреждение, а не отказ.
    checks.push(if !inputs.server_configured {
        SelfTestCheck {
            id: "server",
            label: "Сервер ex_system",
            status: CheckStatus::Warn,
            detail: "Адрес сервера не задан — записи не выгрузятся, пока не настроите."
                .to_string(),
            fix: Some("open_settings"),
        }
    } else if inputs.server_reachable == Some(false) {
        SelfTestCheck {
            id: "server",
            label: "Сервер ex_system",
            status: CheckStatus::Warn,
            detail: "Сервер сейчас недоступен — работа в оффлайне, выгрузка пойдёт позже."
                .to_string(),
            fix: None,
        }
    } else {
        SelfTestCheck {
            id: "server",
            label: "Сервер ex_system",
            status: CheckStatus::Ok,
            detail: "Сервер доступен.".to_string(),
            fix: None,
        }
    });

    // 4. Вход оператора выполнен (если требуется политикой).
    checks.push(if inputs.operator_required && !inputs.operator_present {
        SelfTestCheck {
            id: "operator",
            label: "Вход оператора",
            status: CheckStatus::Fail,
            detail: "Оператор не вошёл — старт записи закрыт. Авторизуйтесь.".to_string(),
            fix: Some("open_login"),
        }
    } else {
        SelfTestCheck {
            id: "operator",
            label: "Вход оператора",
            status: CheckStatus::Ok,
            detail: if inputs.operator_present {
                "Оператор авторизован.".to_string()
            } else {
                "Вход не требуется настройками станции.".to_string()
            },
            fix: None,
        }
    });

    // 5. Ключ станции доступен (R-004, этап 13.5). Без ключа зашифровать ПДн
    //    невозможно: при `encrypt_at_rest` запись сегментов сорвётся → Fail
    //    (блокирует старт); иначе офлайн-вход/админ-PIN недоступны → Warn.
    checks.push(if inputs.station_key_available {
        SelfTestCheck {
            id: "station_key",
            label: "Ключ станции",
            status: CheckStatus::Ok,
            detail: "Ключ станции доступен — шифрование и офлайн-контур работают.".to_string(),
            fix: None,
        }
    } else if inputs.encrypt_at_rest {
        SelfTestCheck {
            id: "station_key",
            label: "Ключ станции",
            status: CheckStatus::Fail,
            detail: "Ключ станции не задан, а шифрование ПДн включено — запись не сохранится. \
                     Задайте ключ станции при развёртывании."
                .to_string(),
            fix: Some("open_settings"),
        }
    } else {
        SelfTestCheck {
            id: "station_key",
            label: "Ключ станции",
            status: CheckStatus::Warn,
            detail: "Ключ станции не задан — офлайн-вход по PIN и админ-PIN недоступны.".to_string(),
            fix: Some("open_settings"),
        }
    });

    // 6. Незавершённых сессий нет.
    checks.push(if inputs.unfinished_count == 0 {
        SelfTestCheck {
            id: "unfinished",
            label: "Незавершённые сессии",
            status: CheckStatus::Ok,
            detail: "Нет незавершённых записей.".to_string(),
            fix: None,
        }
    } else {
        SelfTestCheck {
            id: "unfinished",
            label: "Незавершённые сессии",
            status: CheckStatus::Warn,
            detail: format!(
                "Найдено незавершённых сессий: {}. Продолжите или закройте их на экране «Запись».",
                inputs.unfinished_count
            ),
            fix: Some("open_record"),
        }
    });

    let ready = !checks.iter().any(|c| c.status == CheckStatus::Fail);
    SelfTestReport { checks, ready }
}

/// Best-effort проверка доступности сервера: короткий GET на базовый URL. Любой
/// ответ (даже 4xx/5xx) означает «сервер отвечает»; сетевой сбой/таймаут → false.
fn probe_server(base_url: &str) -> bool {
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(SERVER_PROBE_TIMEOUT_MS))
        .build()
    else {
        return false;
    };
    client.get(base_url.trim_end_matches('/')).send().is_ok()
}

/// Собрать и вернуть self-test-отчёт. Наполняет входы реальным состоянием ядра и
/// делегирует классификацию чистой [`build_report`].
#[tauri::command]
pub fn self_test(app: AppHandle) -> Result<SelfTestReport, String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;

    let devices = list_input_devices().map_err(|e| e.to_string())?;
    let selected_present = match &settings.audio.device {
        // Явно выбранное устройство должно присутствовать среди найденных.
        Some(name) => devices.iter().any(|d| &d.name == name),
        // «Системное по умолчанию» — достаточно наличия любого устройства.
        None => !devices.is_empty(),
    };

    let disk = DiskThresholds {
        low_mb: settings.reliability.disk_low_threshold_mb,
        critical_mb: settings.reliability.disk_critical_mb,
    };
    let free_mb = free_space_mb(&root).unwrap_or(0);

    let server_configured = settings
        .sync
        .server_base_url
        .as_deref()
        .map(|u| !u.trim().is_empty())
        .unwrap_or(false);
    let server_reachable = settings
        .sync
        .server_base_url
        .as_deref()
        .filter(|u| !u.trim().is_empty())
        .map(probe_server);

    let operator_present = app
        .try_state::<AuthState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.is_some()))
        .unwrap_or(false);

    let unfinished_count = recovery::scan_unfinished(&root)
        .map(|v| v.len())
        .unwrap_or(0);

    // R-004: ключ станции проверяем без его использования (диагностика, не расшифровка).
    let station_key_available =
        crate::store::crypto::ensure_station_key(settings.storage.key_source, &root).is_ok();

    let inputs = SelfTestInputs {
        device_count: devices.len(),
        selected_present,
        free_mb,
        disk,
        server_configured,
        server_reachable,
        operator_required: settings.auth.operator.required_to_start,
        operator_present,
        unfinished_count,
        station_key_available,
        encrypt_at_rest: settings.storage.encrypt_at_rest,
    };
    Ok(build_report(&inputs))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISK: DiskThresholds = DiskThresholds {
        low_mb: 1_024,
        critical_mb: 256,
    };

    /// Полностью «зелёные» входы — база для точечных фикстур сбоя.
    fn healthy() -> SelfTestInputs {
        SelfTestInputs {
            device_count: 1,
            selected_present: true,
            free_mb: 5_000,
            disk: DISK,
            server_configured: true,
            server_reachable: Some(true),
            operator_required: true,
            operator_present: true,
            unfinished_count: 0,
            station_key_available: true,
            encrypt_at_rest: true,
        }
    }

    fn check<'a>(r: &'a SelfTestReport, id: &str) -> &'a SelfTestCheck {
        r.checks.iter().find(|c| c.id == id).expect("проверка есть")
    }

    #[test]
    fn all_green_is_ready() {
        let r = build_report(&healthy());
        assert!(r.ready);
        assert!(r.checks.iter().all(|c| c.status == CheckStatus::Ok));
    }

    #[test]
    fn no_device_fails_and_blocks() {
        let mut i = healthy();
        i.device_count = 0;
        let r = build_report(&i);
        assert!(!r.ready);
        assert_eq!(check(&r, "device").status, CheckStatus::Fail);
        assert_eq!(check(&r, "device").fix, Some("open_record"));
    }

    #[test]
    fn missing_selected_device_warns_but_ready() {
        let mut i = healthy();
        i.selected_present = false;
        let r = build_report(&i);
        assert!(r.ready); // предупреждение не блокирует старт
        assert_eq!(check(&r, "device").status, CheckStatus::Warn);
    }

    #[test]
    fn critical_disk_fails() {
        let mut i = healthy();
        i.free_mb = 100; // ниже critical_mb = 256
        let r = build_report(&i);
        assert!(!r.ready);
        assert_eq!(check(&r, "disk").status, CheckStatus::Fail);
    }

    #[test]
    fn low_disk_warns() {
        let mut i = healthy();
        i.free_mb = 512; // между critical и low
        let r = build_report(&i);
        assert!(r.ready);
        assert_eq!(check(&r, "disk").status, CheckStatus::Warn);
    }

    #[test]
    fn offline_server_warns_not_fails() {
        let mut i = healthy();
        i.server_reachable = Some(false);
        let r = build_report(&i);
        assert!(r.ready); // оффлайн-старт допустим
        assert_eq!(check(&r, "server").status, CheckStatus::Warn);
    }

    #[test]
    fn unconfigured_server_warns() {
        let mut i = healthy();
        i.server_configured = false;
        i.server_reachable = None;
        let r = build_report(&i);
        assert!(r.ready);
        assert_eq!(check(&r, "server").status, CheckStatus::Warn);
        assert_eq!(check(&r, "server").fix, Some("open_settings"));
    }

    #[test]
    fn operator_missing_fails_when_required() {
        let mut i = healthy();
        i.operator_present = false;
        let r = build_report(&i);
        assert!(!r.ready);
        assert_eq!(check(&r, "operator").status, CheckStatus::Fail);
        assert_eq!(check(&r, "operator").fix, Some("open_login"));
    }

    #[test]
    fn operator_missing_ok_when_not_required() {
        let mut i = healthy();
        i.operator_present = false;
        i.operator_required = false;
        let r = build_report(&i);
        assert!(r.ready);
        assert_eq!(check(&r, "operator").status, CheckStatus::Ok);
    }

    #[test]
    fn missing_station_key_fails_when_encrypting() {
        // R-004: без ключа при включённом шифровании запись не сохранится → Fail.
        let mut i = healthy();
        i.station_key_available = false;
        i.encrypt_at_rest = true;
        let r = build_report(&i);
        assert!(!r.ready);
        assert_eq!(check(&r, "station_key").status, CheckStatus::Fail);
        assert_eq!(check(&r, "station_key").fix, Some("open_settings"));
    }

    #[test]
    fn missing_station_key_warns_without_encryption() {
        // Шифрование выключено: офлайн-вход/админ-PIN недоступны → Warn, не Fail.
        let mut i = healthy();
        i.station_key_available = false;
        i.encrypt_at_rest = false;
        let r = build_report(&i);
        assert!(r.ready);
        assert_eq!(check(&r, "station_key").status, CheckStatus::Warn);
    }

    #[test]
    fn unfinished_sessions_warn() {
        let mut i = healthy();
        i.unfinished_count = 2;
        let r = build_report(&i);
        assert!(r.ready);
        assert_eq!(check(&r, "unfinished").status, CheckStatus::Warn);
        assert_eq!(check(&r, "unfinished").fix, Some("open_record"));
    }
}
