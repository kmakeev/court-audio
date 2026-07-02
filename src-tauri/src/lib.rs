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

/// Точка сборки Tauri-приложения. Регистрирует IPC-команды и запускает окно.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ipc::audio_cmds::CaptureState::default())
        .manage(ipc::audio_cmds::MonitorState::default())
        .manage(ipc::player_cmds::PlayerState::default())
        .manage(ipc::auth_cmds::AuthState::default())
        .setup(|app| {
            // Фоновый агент выгрузки (этап 06): низкоприоритетный поток, не на
            // горячем пути захвата. Idle, пока не задан sync.server_base_url.
            ipc::sync_cmds::spawn_scheduler(app.handle().clone());
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
            ipc::auth_cmds::auth_login,
            ipc::auth_cmds::auth_logout,
            ipc::auth_cmds::auth_status,
            ipc::auth_cmds::auth_unlock_offline,
            ipc::auth_cmds::auth_reconnect,
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
            ipc::export_cmds::export_session_info,
            ipc::export_cmds::export_build_package,
            ipc::export_cmds::export_dvd_drive_status,
            ipc::export_cmds::export_burn_dvd
        ])
        .run(tauri::generate_context!())
        .expect("ошибка запуска приложения «Аудиопротокол»");
}
