//! Команды интерфейса и адаптивности (этап 10.5).
//!
//! Здесь — управление компакт-окном статуса «поверх всех окон» (Tauri
//! multi-window) и чистая деривация подписи трея по состоянию записи. Сам
//! системный трей собирается в [`crate::run`] (`setup`); эта деривация вынесена
//! отдельной функцией, чтобы покрыть её юнит-тестом без нативного рантайма.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Метка окна компакт-оверлея (одно на приложение).
pub const OVERLAY_LABEL: &str = "overlay";

/// URL окна оверлея: тот же `index.html`, но с меткой окна в query — фронтенд
/// (`main.tsx`) по ней рендерит компактный статус вместо полного приложения.
const OVERLAY_URL: &str = "index.html?window=overlay";

/// Заголовок приложения (для подписи трея).
const APP_TITLE: &str = "Аудиопротокол";

/// Подпись системного трея по состоянию записи (этап 10.5, deliverable 2).
/// Чистая функция — единственная тестируемая точка трея (нативный рендер иконки
/// проверяется вручную по чек-листу `docs/ui_adaptive.md`).
pub fn tray_tooltip(state: &str) -> String {
    let status = match state {
        "recording" => "● Идёт запись",
        "paused" => "❚❚ Пауза",
        "stopping" => "Остановка…",
        "stopped" => "Запись завершена",
        // idle и любое неизвестное состояние — нейтральная готовность.
        _ => "Готов к записи",
    };
    format!("{APP_TITLE} · {status}")
}

/// Открыть компакт-окно статуса «поверх всех окон». Гейт — реестр
/// `ui.compact_overlay.enabled`: при выключенной опции команда отклоняется
/// (кнопка в UI показывается только при включённой опции, но ядро проверяет
/// независимо от UI). Повторный вызов — фокус на уже открытом окне.
#[tauri::command]
pub fn open_compact_overlay(app: AppHandle) -> Result<(), String> {
    let settings = crate::ipc::load_settings(&app)?;
    if !settings.ui.compact_overlay.enabled {
        return Err("Компакт-окно отключено настройкой ui.compact_overlay.enabled".to_string());
    }

    if let Some(win) = app.get_webview_window(OVERLAY_LABEL) {
        win.set_focus()
            .map_err(|e| format!("не удалось активировать окно оверлея: {e}"))?;
        return Ok(());
    }

    WebviewWindowBuilder::new(&app, OVERLAY_LABEL, WebviewUrl::App(OVERLAY_URL.into()))
        .title("Статус записи")
        // Компактно, но с запасом на режим проигрывателя (позиция + транспорт).
        .inner_size(300.0, 210.0)
        .always_on_top(true)
        .resizable(false)
        .skip_taskbar(true)
        // Без системной рамки/кнопок: закрытие — своей кнопкой в окне,
        // перемещение — за drag-полосу в самом окне (`data-tauri-drag-region`).
        .decorations(false)
        .build()
        .map_err(|e| format!("не удалось открыть окно оверлея: {e}"))?;
    Ok(())
}

/// Закрыть компакт-окно статуса (вызывается из UI основного окна и из самого
/// оверлея). Отсутствие окна — не ошибка (идемпотентно).
#[tauri::command]
pub fn close_compact_overlay(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(OVERLAY_LABEL) {
        win.close()
            .map_err(|e| format!("не удалось закрыть окно оверлея: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_tooltip_covers_all_states() {
        assert_eq!(tray_tooltip("idle"), "Аудиопротокол · Готов к записи");
        assert_eq!(tray_tooltip("recording"), "Аудиопротокол · ● Идёт запись");
        assert_eq!(tray_tooltip("paused"), "Аудиопротокол · ❚❚ Пауза");
        assert_eq!(tray_tooltip("stopping"), "Аудиопротокол · Остановка…");
        assert_eq!(tray_tooltip("stopped"), "Аудиопротокол · Запись завершена");
        // Неизвестное состояние падает в нейтральную готовность (не паникует).
        assert_eq!(tray_tooltip("bogus"), "Аудиопротокол · Готов к записи");
    }
}
