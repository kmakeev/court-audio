//! Гейт сохранения настроек: разграничение оператор/админ, опасные изменения,
//! поле-уровневый diff (этап 10.4 — `promts/10_4_settings_roles.md`, шаг 1).
//!
//! **Чистая логика без Tauri/IO** (как `auth_cmds::start_allowed`,
//! `export::check_policy`) — гейт обязан работать на уровне ядра, а не только в
//! UI, и тестируется офлайн. Команда сохранения ([`super::save_settings`]) —
//! тонкая обвязка: читает текущие настройки, спрашивает [`authorize_save`],
//! пишет файл и журнал ([`crate::store::settings_audit`]).
//!
//! **Область доступа** (реестр `configuration.md` → «Область доступа»):
//! оператор-скоуп = `audio.device`, `audio.roles`, `markers.categories`,
//! `player.*`; всё остальное — админ.

use crate::settings::Settings;
use crate::store::settings_audit::FieldChange;

/// Категория «опасного» изменения (реестр `configuration.md` → «Администрирование»).
/// Требует явного подтверждения оператора и помечает запись журнала.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DangerKind {
    /// Смена адреса сервера `ex_system` (выгрузка уходит в другую систему).
    ServerUrl,
    /// Отключение шифрования at-rest (ПДн ложатся на диск открытыми).
    DisableEncryption,
    /// Смягчение ретеншна (дольше держим локальные ПДн / слабее условие удаления).
    LoosenRetention,
}

impl DangerKind {
    /// Человекочитаемый ярлык (для диалога подтверждения в UI).
    pub fn label(self) -> &'static str {
        match self {
            DangerKind::ServerUrl => "Смена адреса сервера ex_system",
            DangerKind::DisableEncryption => "Отключение шифрования записей",
            DangerKind::LoosenRetention => "Смягчение политики хранения (ретеншн)",
        }
    }
}

/// Итог авторизации сохранения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveDecision {
    /// Можно сохранять; `dangerous` — какие опасные изменения подтверждены (для журнала).
    Allow { dangerous: Vec<DangerKind> },
    /// Есть админ-изменение, но прав администратора нет — отказ на уровне ядра.
    NeedAdmin,
    /// Есть опасное изменение без подтверждения — требуется явное подтверждение.
    NeedConfirmation { dangerous: Vec<DangerKind> },
}

/// Затронуты ли **только** оператор-скоуп поля. Накладывает оператор-скоуп
/// `incoming` на клон `current`: если результат совпал с `incoming` — все
/// админ-поля `incoming` уже равны `current`, т.е. изменения (если есть) лежат
/// в оператор-скоупе. Опирается на derive `PartialEq` для `Settings`.
pub fn only_operator_changed(current: &Settings, incoming: &Settings) -> bool {
    let mut probe = current.clone();
    probe.audio.device = incoming.audio.device.clone();
    probe.audio.roles = incoming.audio.roles.clone();
    probe.markers.categories = incoming.markers.categories.clone();
    probe.player = incoming.player.clone();
    &probe == incoming
}

/// Опасные изменения между `current` и `incoming`. Все они — админ-скоуп,
/// поэтому при чисто-операторском изменении список пуст.
pub fn dangerous_changes(current: &Settings, incoming: &Settings) -> Vec<DangerKind> {
    let mut out = Vec::new();
    if current.sync.server_base_url != incoming.sync.server_base_url {
        out.push(DangerKind::ServerUrl);
    }
    if current.storage.encrypt_at_rest && !incoming.storage.encrypt_at_rest {
        out.push(DangerKind::DisableEncryption);
    }
    let retention_loosened = current.retention.mode != incoming.retention.mode
        || (current.retention.require_integrity_verified
            && !incoming.retention.require_integrity_verified);
    if retention_loosened {
        out.push(DangerKind::LoosenRetention);
    }
    out
}

/// Авторизовать сохранение. Порядок: (1) админ-изменение без прав → `NeedAdmin`;
/// (2) опасное изменение без подтверждения → `NeedConfirmation`; (3) `Allow`.
pub fn authorize_save(
    current: &Settings,
    incoming: &Settings,
    admin_unlocked: bool,
    admin_required: bool,
    confirm_dangerous: bool,
) -> SaveDecision {
    let admin_change = !only_operator_changed(current, incoming);
    if admin_change && admin_required && !admin_unlocked {
        return SaveDecision::NeedAdmin;
    }
    let dangerous = dangerous_changes(current, incoming);
    if !dangerous.is_empty() && !confirm_dangerous {
        return SaveDecision::NeedConfirmation { dangerous };
    }
    SaveDecision::Allow { dangerous }
}

/// Поле-уровневый diff двух конфигов (`serde_json::Value`): плоские dotted-пути
/// по различающимся листам, со старым и новым значением. Массивы/скаляры —
/// целые листы; объекты — рекурсия по объединению ключей. Секретов в `Settings`
/// нет, поэтому редактирования значений не требуется.
pub fn diff_json(old: &serde_json::Value, new: &serde_json::Value) -> Vec<FieldChange> {
    let mut out = Vec::new();
    diff_into(String::new(), old, new, &mut out);
    out
}

fn diff_into(
    prefix: String,
    old: &serde_json::Value,
    new: &serde_json::Value,
    out: &mut Vec<FieldChange>,
) {
    use serde_json::Value;
    match (old, new) {
        (Value::Object(o), Value::Object(n)) => {
            // Объединение ключей: покрываем и добавленные, и убранные поля.
            let mut keys: Vec<&String> = o.keys().chain(n.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let child = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                let ov = o.get(k).unwrap_or(&Value::Null);
                let nv = n.get(k).unwrap_or(&Value::Null);
                diff_into(child, ov, nv, out);
            }
        }
        _ => {
            if old != new {
                out.push(FieldChange {
                    path: prefix,
                    old: old.clone(),
                    new: new.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Разграничение оператор/админ ──────────────────────────────────────────

    #[test]
    fn operator_scope_changes_are_not_admin() {
        let cur = Settings::default();
        // Устройство, роли, категории меток, плеер — оператор-скоуп.
        let mut inc = cur.clone();
        inc.audio.device = Some("USB Mic".into());
        assert!(only_operator_changed(&cur, &inc));

        let mut inc = cur.clone();
        inc.audio.roles = vec!["judge".into(), "clerk".into()];
        assert!(only_operator_changed(&cur, &inc));

        let mut inc = cur.clone();
        inc.markers.categories = vec!["Закладка".into()];
        assert!(only_operator_changed(&cur, &inc));

        let mut inc = cur.clone();
        inc.player.seek_step_seconds = 30.0;
        assert!(only_operator_changed(&cur, &inc));
    }

    #[test]
    fn admin_scope_changes_are_detected() {
        let cur = Settings::default();
        let mut inc = cur.clone();
        inc.sync.server_base_url = Some("https://ex.example".into());
        assert!(!only_operator_changed(&cur, &inc));

        let mut inc = cur.clone();
        inc.storage.encrypt_at_rest = false;
        assert!(!only_operator_changed(&cur, &inc));

        let mut inc = cur.clone();
        inc.audio.sample_rate_hz = 48_000; // качество аудио — админ
        assert!(!only_operator_changed(&cur, &inc));

        let mut inc = cur.clone();
        inc.audio.multichannel.enabled = true; // карта дорожек — админ
        assert!(!only_operator_changed(&cur, &inc));
    }

    #[test]
    fn no_change_is_not_admin() {
        let cur = Settings::default();
        assert!(only_operator_changed(&cur, &cur.clone()));
    }

    // ── Опасные изменения ─────────────────────────────────────────────────────

    #[test]
    fn dangerous_detects_server_encryption_retention() {
        let cur = Settings::default();

        let mut inc = cur.clone();
        inc.sync.server_base_url = Some("https://other".into());
        assert_eq!(dangerous_changes(&cur, &inc), vec![DangerKind::ServerUrl]);

        let mut inc = cur.clone();
        inc.storage.encrypt_at_rest = false;
        assert_eq!(
            dangerous_changes(&cur, &inc),
            vec![DangerKind::DisableEncryption]
        );

        let mut inc = cur.clone();
        inc.retention.require_integrity_verified = false;
        assert_eq!(
            dangerous_changes(&cur, &inc),
            vec![DangerKind::LoosenRetention]
        );

        let mut inc = cur.clone();
        inc.retention.mode = crate::settings::RetentionMode::Manual;
        assert_eq!(
            dangerous_changes(&cur, &inc),
            vec![DangerKind::LoosenRetention]
        );
    }

    #[test]
    fn enabling_encryption_is_not_dangerous() {
        let mut cur = Settings::default();
        cur.storage.encrypt_at_rest = false;
        let mut inc = cur.clone();
        inc.storage.encrypt_at_rest = true; // включение — безопасно
        assert!(dangerous_changes(&cur, &inc).is_empty());
    }

    #[test]
    fn non_dangerous_admin_change_has_no_flags() {
        let cur = Settings::default();
        let mut inc = cur.clone();
        inc.sync.chunk_size_mb = 16; // админ, но не опасно
        assert!(dangerous_changes(&cur, &inc).is_empty());
    }

    // ── Авторизация ───────────────────────────────────────────────────────────

    #[test]
    fn operator_change_allowed_when_admin_locked() {
        let cur = Settings::default();
        let mut inc = cur.clone();
        inc.audio.device = Some("Mic".into());
        assert_eq!(
            authorize_save(&cur, &inc, false, true, false),
            SaveDecision::Allow { dangerous: vec![] }
        );
    }

    #[test]
    fn admin_change_denied_when_locked() {
        let cur = Settings::default();
        let mut inc = cur.clone();
        inc.sync.chunk_size_mb = 16;
        assert_eq!(
            authorize_save(&cur, &inc, false, true, false),
            SaveDecision::NeedAdmin
        );
    }

    #[test]
    fn admin_change_allowed_when_unlocked_non_dangerous() {
        let cur = Settings::default();
        let mut inc = cur.clone();
        inc.sync.chunk_size_mb = 16;
        assert_eq!(
            authorize_save(&cur, &inc, true, true, false),
            SaveDecision::Allow { dangerous: vec![] }
        );
    }

    #[test]
    fn dangerous_needs_confirmation_then_allowed() {
        let cur = Settings::default();
        let mut inc = cur.clone();
        inc.sync.server_base_url = Some("https://ex".into());
        // Разблокировано, но без подтверждения — просят подтвердить.
        assert_eq!(
            authorize_save(&cur, &inc, true, true, false),
            SaveDecision::NeedConfirmation {
                dangerous: vec![DangerKind::ServerUrl]
            }
        );
        // С подтверждением — разрешаем, опасные фиксируем в журнале.
        assert_eq!(
            authorize_save(&cur, &inc, true, true, true),
            SaveDecision::Allow {
                dangerous: vec![DangerKind::ServerUrl]
            }
        );
    }

    #[test]
    fn admin_gate_bypassed_when_not_required() {
        let cur = Settings::default();
        let mut inc = cur.clone();
        inc.sync.chunk_size_mb = 16; // админ, но гейт выключен
        assert_eq!(
            authorize_save(&cur, &inc, false, false, false),
            SaveDecision::Allow { dangerous: vec![] }
        );
    }

    // ── diff_json ─────────────────────────────────────────────────────────────

    #[test]
    fn diff_json_reports_leaf_paths() {
        let cur = Settings::default();
        let mut inc = cur.clone();
        inc.sync.server_base_url = Some("https://ex.example".into());
        let old = serde_json::to_value(&cur).unwrap();
        let new = serde_json::to_value(&inc).unwrap();
        let diff = diff_json(&old, &new);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].path, "sync.server_base_url");
        assert_eq!(diff[0].old, serde_json::Value::Null);
        assert_eq!(diff[0].new, serde_json::json!("https://ex.example"));
    }

    #[test]
    fn diff_json_empty_when_equal() {
        let cur = serde_json::to_value(Settings::default()).unwrap();
        assert!(diff_json(&cur, &cur).is_empty());
    }
}
