//! Ядро захвата «Аудиопротокол».
//!
//! Структура модулей повторяет карту из `CLAUDE.md`/`promts/`: каждый модуль —
//! заготовка профильного этапа (01–06). На этапе 00 рабочей логики нет — только
//! каркас, модель [`settings::Settings`] и IPC-команды настроек.

pub mod audio;
pub mod export;
pub mod integrity;
pub mod ipc;
pub mod player;
pub mod recorder;
pub mod reliability;
pub mod settings;
pub mod store;
pub mod sync;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Listener, Manager};

/// Собрать системный трей со статусом записи (этап 10.5). Иконка — штатная иконка
/// окна; tooltip меняется по событию `capture_state`. Меню: показать окно / выход.
/// Деривация подписи — чистая [`ipc::ui_cmds::tray_tooltip`] (покрыта тестом);
/// здесь только нативная обвязка (в CI без дисплея не исполняется).
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Показать окно", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip(ipc::ui_cmds::tray_tooltip("idle"))
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;

    // Обновление подписи трея по состоянию записи: слушаем `capture_state` и
    // переносим состояние в tooltip (запись видна из трея с любого экрана).
    let handle = app.handle().clone();
    app.listen("capture_state", move |event| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) {
            if let Some(state) = v.get("state").and_then(|s| s.as_str()) {
                if let Some(tray) = handle.tray_by_id("main") {
                    let _ = tray.set_tooltip(Some(ipc::ui_cmds::tray_tooltip(state)));
                }
            }
        }
    });
    Ok(())
}

/// Точка сборки Tauri-приложения. Регистрирует IPC-команды и запускает окно.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ipc::audio_cmds::CaptureState::default())
        .manage(ipc::audio_cmds::MonitorState::default())
        .manage(ipc::player_cmds::PlayerState::default())
        .manage(ipc::auth_cmds::AuthState::default())
        .manage(ipc::admin_cmds::AdminState::default())
        .setup(|app| {
            // Системный трей со статусом записи (этап 10.5). Не критичен для
            // старта: сбой трея (например, окружение без дисплея) не роняет запуск.
            if let Err(e) = setup_tray(app) {
                eprintln!("трей недоступен: {e}");
            }
            // Фоновый агент выгрузки (этап 06): низкоприоритетный поток, не на
            // горячем пути захвата. Idle, пока не задан sync.server_base_url.
            ipc::sync_cmds::spawn_scheduler(app.handle().clone());
            // Валидация конфигурации и ключа станции на старте (этап 13.5,
            // R-003/R-004). Диагностика громкая: молчаливая деградация —
            // именно тот дефект, который здесь закрываем.
            let handle = app.handle().clone();
            match ipc::load_settings(&handle) {
                Err(e) => {
                    // R-003: битый settings.json не валит процесс, но гейты
                    // применяются fail-secure (см. ipc::load_settings).
                    eprintln!("ВНИМАНИЕ: {e}");
                }
                Ok(settings) => {
                    if let Ok(root) = ipc::resolve_storage_root(&handle, &settings) {
                        // R-004: явная валидация ключа станции. Без ключа офлайн-
                        // вход/админ-PIN/шифрование ПДн работать не будут —
                        // диагностируем громко, а не деградируем молча.
                        match store::crypto::ensure_station_key(settings.storage.key_source, &root) {
                            Err(e) => eprintln!(
                                "ВНИМАНИЕ: ключ станции недоступен ({e}). Офлайн-вход, \
                                 админ-PIN и шифрование ПДн недоступны. Задайте {} при \
                                 развёртывании (см. docs/packaging.md).",
                                store::crypto::PASSPHRASE_ENV
                            ),
                            // Ключ есть — провижиним админ-PIN из env (этап 10.4):
                            // на первом запуске засеивает хеш в зашифрованный блоб.
                            Ok(()) => {
                                if let Err(e) = store::admin_pin::provision_from_env_if_absent(
                                    &root,
                                    settings.storage.key_source,
                                    settings.admin.pin.min_length,
                                ) {
                                    eprintln!("ВНИМАНИЕ: не удалось провизионировать админ-PIN: {e}");
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::get_settings,
            ipc::save_settings,
            ipc::audio_cmds::list_audio_devices,
            ipc::audio_cmds::start_capture,
            ipc::audio_cmds::stop_capture,
            ipc::audio_cmds::pause_capture,
            ipc::audio_cmds::resume_capture,
            ipc::audio_cmds::capture_status,
            ipc::audio_cmds::start_monitor,
            ipc::audio_cmds::stop_monitor,
            ipc::audio_cmds::scan_recoverable,
            ipc::audio_cmds::recover_session,
            ipc::audio_cmds::discard_session,
            ipc::query_cmds::list_sessions,
            ipc::query_cmds::diagnostics,
            ipc::query_cmds::session_detail,
            ipc::query_cmds::set_session_comment,
            ipc::selftest_cmds::self_test,
            ipc::auth_cmds::auth_login,
            ipc::auth_cmds::auth_logout,
            ipc::auth_cmds::auth_status,
            ipc::auth_cmds::auth_unlock_offline,
            ipc::auth_cmds::auth_reconnect,
            ipc::admin_cmds::admin_status,
            ipc::admin_cmds::admin_unlock,
            ipc::admin_cmds::admin_lock,
            ipc::admin_cmds::get_settings_audit,
            ipc::admin_cmds::export_station_profile,
            ipc::admin_cmds::import_station_profile,
            ipc::case_cmds::get_case_cache_status,
            ipc::case_cmds::search_cases,
            ipc::case_cmds::sync_case_cache,
            ipc::case_cmds::bind_session_case,
            ipc::sync_cmds::retry_upload,
            ipc::sync_cmds::pause_upload,
            ipc::sync_cmds::resume_upload,
            ipc::marker_cmds::add_marker,
            ipc::marker_cmds::edit_marker,
            ipc::marker_cmds::remove_marker,
            ipc::marker_cmds::start_role_span,
            ipc::marker_cmds::end_role_span,
            ipc::marker_cmds::list_annotations,
            ipc::player_cmds::player_open_session,
            ipc::player_cmds::player_select_track,
            ipc::player_cmds::player_play,
            ipc::player_cmds::player_pause,
            ipc::player_cmds::player_seek,
            ipc::player_cmds::player_set_rate,
            ipc::player_cmds::player_set_volume,
            ipc::player_cmds::player_close,
            ipc::player_cmds::player_status,
            ipc::export_cmds::export_session_info,
            ipc::export_cmds::export_build_package,
            ipc::export_cmds::export_dvd_drive_status,
            ipc::export_cmds::export_burn_dvd,
            ipc::ui_cmds::open_compact_overlay,
            ipc::ui_cmds::close_compact_overlay
        ])
        .run(tauri::generate_context!())
        .expect("ошибка запуска приложения «Аудиопротокол»");
}
