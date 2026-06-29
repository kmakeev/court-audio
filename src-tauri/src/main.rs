// Скрываем дополнительное консольное окно на Windows в release-сборке.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    court_audio_lib::run()
}
