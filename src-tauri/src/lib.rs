//! Ядро захвата «Аудиопротокол».
//!
//! Структура модулей повторяет карту из `CLAUDE.md`/`promts/`: каждый модуль —
//! заготовка профильного этапа (01–06). На этапе 00 рабочей логики нет — только
//! каркас, модель [`settings::Settings`] и IPC-команды настроек.

pub mod audio;
pub mod integrity;
pub mod ipc;
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
            ipc::case_cmds::get_case_cache_status,
            ipc::case_cmds::search_cases,
            ipc::case_cmds::sync_case_cache,
            ipc::case_cmds::bind_session_case
        ])
        .run(tauri::generate_context!())
        .expect("ошибка запуска приложения «Аудиопротокол»");
}
