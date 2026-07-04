//! Tauri-команды администрирования станции (этап 10.4 —
//! `promts/10_4_settings_roles.md`, шаги 1, 3–5).
//!
//! Права администратора в v1 — **валидный админ-PIN** (оффлайн-фолбэк; роль-из-
//! `ex_system` отложена). PIN проверяется против зашифрованного блоба
//! ([`crate::store::admin_pin`]); после успешной разблокировки в памяти ядра
//! держится флаг [`AdminState`] (сбрасывается при рестарте/выходе). Гейт
//! сохранения ([`apply_settings_save`]) на уровне ядра сверяет изменения с
//! разграничением оператор/админ ([`super::settings_gate`]), подтверждает
//! опасные изменения и пишет журнал ([`crate::store::settings_audit`]).
//!
//! Экспорт/импорт **профиля станции** — полный `Settings` в JSON (секретов там
//! нет). Импорт проходит тот же гейт (всегда как админ-изменение) и журналируется.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::ipc::auth_cmds::current_operator_id;
use crate::ipc::settings_gate::{authorize_save, diff_json, only_operator_changed, SaveDecision};
use crate::ipc::{load_settings, resolve_storage_root, write_settings, MANIFEST_FILE};
use crate::reliability::watchdog::now_unix_ms;
use crate::settings::Settings;
use crate::store::manifest::ManifestStore;
use crate::store::settings_audit::{self, ChangeSource, SettingsChange};
use crate::store::{admin_pin, StoreError};

/// Разблокирован ли админ-доступ в текущем сеансе (в памяти ядра). Сбрасывается
/// при рестарте приложения и по `admin_lock`.
#[derive(Default)]
pub struct AdminState(pub Mutex<bool>);

/// Снимок статуса админ-доступа для UI.
#[derive(Debug, Clone, Serialize)]
pub struct AdminStatusView {
    /// Задан ли админ-PIN при развёртывании (иначе админ-изменения невозможны).
    pub provisioned: bool,
    /// Разблокирован ли админ-доступ в текущем сеансе.
    pub unlocked: bool,
    /// Требуется ли админ-PIN политикой (`admin.pin.required`).
    pub required: bool,
}

/// Итог сохранения настроек для UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SaveOutcome {
    /// Сохранено (и, при `integrity.event_log`, записано в журнал).
    Saved,
    /// Есть опасные изменения — нужен явный повтор с подтверждением.
    NeedsConfirmation { dangerous: Vec<String> },
}

/// Fail-secure проверка админ-гейта по **результату загрузки настроек** (R-003,
/// этап 13.5). Повреждённый конфиг (`Err`) НИКОГДА не размыкает гейт: любое
/// изменение отклоняется, чтобы порча `settings.json` не давала обойти админ-PIN.
/// При корректном конфиге судим по текущей политике `admin.pin.required`. Чистая
/// функция (тестируется без Tauri).
pub fn admin_change_denied(
    config: Result<&Settings, &str>,
    admin_change: bool,
    unlocked: bool,
) -> bool {
    match config {
        Ok(settings) => admin_change && settings.admin.pin.required && !unlocked,
        // Битый конфиг: не доверяем политике — смыкаем гейт.
        Err(_) => true,
    }
}

// ── Помощники ─────────────────────────────────────────────────────────────────

fn admin_root_and_key(app: &AppHandle) -> Result<(std::path::PathBuf, crate::settings::KeySource), String> {
    let settings = load_settings(app)?;
    let root = resolve_storage_root(app, &settings)?;
    Ok((root, settings.storage.key_source))
}

fn status_view(app: &AppHandle, admin: &AdminState) -> Result<AdminStatusView, String> {
    let settings = load_settings(app)?;
    let root = resolve_storage_root(app, &settings)?;
    let unlocked = *admin.0.lock().map_err(|_| "состояние админ-доступа повреждено".to_string())?;
    Ok(AdminStatusView {
        provisioned: admin_pin::is_provisioned(&root),
        unlocked,
        required: settings.admin.pin.required,
    })
}

/// Применить сохранение настроек через гейт разграничения доступа. Общий путь
/// для ручного сохранения ([`super::save_settings`]) и импорта профиля. При
/// `force_admin` (импорт) изменение всегда считается админским.
pub(crate) fn apply_settings_save(
    app: &AppHandle,
    admin: &AdminState,
    incoming: Settings,
    source: ChangeSource,
    force_admin: bool,
    confirm_dangerous: bool,
) -> Result<SaveOutcome, String> {
    // Fail-secure: битый `settings.json` отсекается здесь тем же сообщением
    // «конфигурация повреждена» (не размыкает гейт) — R-003.
    let current = load_settings(app)?;
    // Гейт судит по **текущей** политике (нельзя ослабить гейт тем же сохранением).
    let admin_required = current.admin.pin.required;
    let unlocked = *admin.0.lock().map_err(|_| "состояние админ-доступа повреждено".to_string())?;
    let root = resolve_storage_root(app, &current)?;

    // Импорт — всегда админ-действие: даже профиль, отличающийся лишь оператор-
    // полями, не должен применяться в обход админ-гейта (deliverable 5).
    let admin_change = force_admin || !only_operator_changed(&current, &incoming);
    if admin_change_denied(Ok(&current), admin_change, unlocked) {
        return Err(admin_denied_message(&root));
    }

    let decision = authorize_save(&current, &incoming, unlocked, admin_required, confirm_dangerous);
    let dangerous = match decision {
        SaveDecision::NeedAdmin => return Err(admin_denied_message(&root)),
        SaveDecision::NeedConfirmation { dangerous } => {
            return Ok(SaveOutcome::NeedsConfirmation {
                dangerous: dangerous.iter().map(|d| d.label().to_string()).collect(),
            })
        }
        SaveDecision::Allow { dangerous } => dangerous,
    };

    // Персист файла — durable-действие раньше журнала.
    write_settings(app, &incoming)?;

    // Журнал изменений (если включён): кто/когда/что (старое→новое, без секретов).
    if current.integrity.event_log {
        let old = serde_json::to_value(&current).map_err(|e| e.to_string())?;
        let new = serde_json::to_value(&incoming).map_err(|e| e.to_string())?;
        let changes = diff_json(&old, &new);
        if !changes.is_empty() {
            let change = SettingsChange {
                at_unix_ms: now_unix_ms(),
                actor_operator_id: current_operator_id(app),
                source,
                dangerous: !dangerous.is_empty(),
                changes,
            };
            record_change(&root, &change).map_err(|e| e.to_string())?;
        }
    }

    // Оповестить UI об изменении настроек: индикаторы/режимы, читающие реестр
    // (например, флаги `ui.*` в оболочке), должны обновиться без перезапуска.
    let _ = app.emit("settings_saved", ());

    Ok(SaveOutcome::Saved)
}

fn record_change(root: &std::path::Path, change: &SettingsChange) -> Result<(), StoreError> {
    let store = ManifestStore::open(&root.join(MANIFEST_FILE))?;
    settings_audit::record(&store, change)?;
    Ok(())
}

/// Понятное сообщение отказа: не задан PIN vs просто не разблокирован.
fn admin_denied_message(root: &std::path::Path) -> String {
    if admin_pin::is_provisioned(root) {
        "изменение админ-настроек требует прав администратора — разблокируйте админ-доступ PIN".to_string()
    } else {
        format!(
            "админ-PIN не задан при развёртывании (env {}) — изменение админ-настроек невозможно",
            admin_pin::ADMIN_PIN_ENV
        )
    }
}

// ── Команды ───────────────────────────────────────────────────────────────────

/// Текущий статус админ-доступа (UI читает при монтировании экрана).
#[tauri::command]
pub fn admin_status(app: AppHandle, admin: State<'_, AdminState>) -> Result<AdminStatusView, String> {
    status_view(&app, &admin)
}

/// Разблокировать админ-доступ по PIN: сверка с зашифрованным блобом. Неверный
/// PIN → `Err`; не задан PIN → `Err` с подсказкой про развёртывание.
#[tauri::command]
pub fn admin_unlock(
    app: AppHandle,
    admin: State<'_, AdminState>,
    pin: String,
) -> Result<AdminStatusView, String> {
    let (root, key_source) = admin_root_and_key(&app)?;
    if !admin_pin::is_provisioned(&root) {
        return Err(admin_denied_message(&root));
    }
    let ok = admin_pin::verify(&root, key_source, pin.trim()).map_err(|e| e.to_string())?;
    if !ok {
        return Err("неверный админ-PIN".to_string());
    }
    *admin.0.lock().map_err(|_| "состояние админ-доступа повреждено".to_string())? = true;
    status_view(&app, &admin)
}

/// Заблокировать админ-доступ (снять разблокировку в текущем сеансе).
#[tauri::command]
pub fn admin_lock(app: AppHandle, admin: State<'_, AdminState>) -> Result<AdminStatusView, String> {
    *admin.0.lock().map_err(|_| "состояние админ-доступа повреждено".to_string())? = false;
    status_view(&app, &admin)
}

/// Журнал изменений настроек (новейшие сверху, не более `limit`) — для экрана
/// администрирования. Пустой манифест → пустой список.
#[tauri::command]
pub fn get_settings_audit(
    app: AppHandle,
    limit: u32,
) -> Result<Vec<settings_audit::SettingsAuditRecord>, String> {
    let settings = load_settings(&app)?;
    let root = resolve_storage_root(&app, &settings)?;
    let store = ManifestStore::open(&root.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;
    store.list_settings_audit(limit).map_err(|e| e.to_string())
}

/// Экспорт профиля станции: полный `Settings` в JSON (секретов нет). Доступен
/// администратору — как и остальные операции раздела: при активном гейте
/// (`admin.pin.required`) требует разблокировки, иначе (гейт выключен) открыт.
#[tauri::command]
pub fn export_station_profile(
    app: AppHandle,
    admin: State<'_, AdminState>,
) -> Result<String, String> {
    let settings = load_settings(&app)?;
    let unlocked = *admin.0.lock().map_err(|_| "состояние админ-доступа повреждено".to_string())?;
    if settings.admin.pin.required && !unlocked {
        let root = resolve_storage_root(&app, &settings)?;
        return Err(admin_denied_message(&root));
    }
    serde_json::to_string_pretty(&settings).map_err(|e| format!("не удалось собрать профиль: {e}"))
}

/// Импорт профиля станции: разбор JSON → тот же гейт сохранения (всегда как
/// админ-изменение) + журнал. Идемпотентен: повторный импорт того же профиля
/// даёт идентичную конфигурацию.
#[tauri::command]
pub fn import_station_profile(
    app: AppHandle,
    admin: State<'_, AdminState>,
    profile_json: String,
    confirm_dangerous: bool,
) -> Result<SaveOutcome, String> {
    let incoming: Settings = serde_json::from_str(&profile_json)
        .map_err(|e| format!("не удалось разобрать профиль станции: {e}"))?;
    apply_settings_save(&app, &admin, incoming, ChangeSource::Import, true, confirm_dangerous)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn station_profile_export_import_is_idempotent_and_secretless() {
        // Профиль = полный Settings в JSON; секретов в Settings нет по построению
        // (ключи/PIN/пароли живут в env/зашифрованных блобах, не в конфиге).
        let mut s = Settings::default();
        s.sync.server_base_url = Some("https://ex.example".into());
        s.audio.roles = vec!["judge".into(), "clerk".into()];
        let json = serde_json::to_string_pretty(&s).unwrap();
        // Ни одного «секретного» ключа в выгруженном профиле.
        for secret in ["passphrase", "pin_hash", "refresh_token", "access_token", "password"] {
            assert!(!json.contains(secret), "профиль не должен содержать {secret}");
        }
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn save_outcome_serializes_tagged() {
        let json = serde_json::to_string(&SaveOutcome::Saved).unwrap();
        assert_eq!(json, "{\"kind\":\"saved\"}");
        let json = serde_json::to_string(&SaveOutcome::NeedsConfirmation {
            dangerous: vec!["Смена адреса сервера ex_system".into()],
        })
        .unwrap();
        assert!(json.contains("\"kind\":\"needs_confirmation\""));
        assert!(json.contains("Смена адреса сервера"));
    }
}
