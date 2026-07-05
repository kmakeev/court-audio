//! Команды интерфейса и адаптивности (этап 10.5).
//!
//! Здесь — управление компакт-окном статуса «поверх всех окон» (Tauri
//! multi-window) и чистая деривация подписи трея по состоянию записи. Сам
//! системный трей собирается в [`crate::run`] (`setup`); эта деривация вынесена
//! отдельной функцией, чтобы покрыть её юнит-тестом без нативного рантайма.

use tauri::window::Color;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Метка окна компакт-оверлея (одно на приложение).
pub const OVERLAY_LABEL: &str = "overlay";

/// URL окна оверлея: тот же `index.html`, но с меткой окна в query — фронтенд
/// (`main.tsx`) по ней рендерит компактный статус вместо полного приложения.
const OVERLAY_URL: &str = "index.html?window=overlay";

/// Заголовок приложения (для подписи трея).
const APP_TITLE: &str = "Аудиопротокол";

/// Фон окна оверлея — тёмная тема дизайн-системы (`src/styles/tokens.css`,
/// `--dark: #1f1c16` → RGB 31/28/22). **R-006 (этап 13.6):** на Windows WebView2
/// до первого рендера показывал **белую рамку** (дефолтный белый фон вьюпорта).
/// Явный фон окна **и** webview-слоя (Tauri `background_color` красит оба)
/// закрашивает вьюпорт в тёмный ещё до загрузки CSS — вспышки белого нет. Это
/// косметическая константа кода (токен дизайн-системы), не бизнес-параметр
/// реестра — как layout-константы метров (см. `configuration.md`, раздел 10.5).
const OVERLAY_BG: Color = Color(0x1f, 0x1c, 0x16, 0xff);

/// Спецификация окна оверлея — чистое описание конфигурации, вынесенное из
/// команды ради юнит-теста (нативная сборка окна в CI без дисплея не исполняется,
/// как трей; см. `docs/ui_adaptive.md`). Значения размера/фона — косметические
/// константы кода, флаги — поведение окна.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlaySpec {
    pub label: &'static str,
    pub url: &'static str,
    pub width: f64,
    pub height: f64,
    pub background: Color,
    pub always_on_top: bool,
    pub resizable: bool,
    pub skip_taskbar: bool,
    pub decorations: bool,
    /// **R-006 (вторичное).** Окно создаётся **без захвата фокуса**: оверлей —
    /// пассивный индикатор поверх всех окон, фокус остаётся у основного окна, чтобы
    /// новое `always_on_top`-окно не перехватывало ввод. Первопричина «зависания
    /// старт/стопа» — не фокус, а синхронная команда создания окна (см. doc у
    /// [`open_compact_overlay`]); `focused:false` — гигиена поверх основного фикса.
    pub focused: bool,
}

/// Конфигурация окна оверлея (единая точка правды для команды и теста).
pub fn overlay_spec() -> OverlaySpec {
    OverlaySpec {
        label: OVERLAY_LABEL,
        url: OVERLAY_URL,
        // Компактно, но с запасом на режим проигрывателя (позиция + транспорт).
        width: 300.0,
        height: 210.0,
        background: OVERLAY_BG,
        always_on_top: true,
        resizable: false,
        skip_taskbar: true,
        // Без системной рамки/кнопок: закрытие — своей кнопкой в окне,
        // перемещение — за drag-полосу в самом окне (`data-tauri-drag-region`).
        decorations: false,
        focused: false,
    }
}

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
///
/// **R-006 (корень дефекта Windows).** Команда обязана быть `async`: в Tauri v2
/// создание окна (`WebviewWindowBuilder::build`) из **синхронной** команды
/// **дедлочит цикл событий на Windows и Linux** (на macOS — нет, потому дефект и
/// не воспроизводился в разработке). Именно этот дедлок давал наблюдаемую картину
/// на станции: окно-оверлей появлялось тёмным «квадратом» без данных (JS не
/// исполнялся, пока цикл заблокирован), а всё приложение переставало отвечать —
/// закрыть можно было только снятием задачи. Async-команда исполняется отдельной
/// задачей, а сборку окна Tauri проксирует в главный поток без блокировки. Тёмный
/// фон (`OVERLAY_BG`) и `focused:false` — вторичные правки (косметика/фокус), корень
/// — асинхронность.
#[tauri::command]
pub async fn open_compact_overlay(app: AppHandle) -> Result<(), String> {
    let settings = crate::ipc::load_settings(&app)?;
    if !settings.ui.compact_overlay.enabled {
        return Err("Компакт-окно отключено настройкой ui.compact_overlay.enabled".to_string());
    }

    if let Some(win) = app.get_webview_window(OVERLAY_LABEL) {
        win.set_focus()
            .map_err(|e| format!("не удалось активировать окно оверлея: {e}"))?;
        return Ok(());
    }

    let spec = overlay_spec();
    WebviewWindowBuilder::new(&app, spec.label, WebviewUrl::App(spec.url.into()))
        .title("Статус записи")
        .inner_size(spec.width, spec.height)
        // R-006: тёмный фон окна+webview убирает белую рамку WebView2 до рендера.
        .background_color(spec.background)
        .always_on_top(spec.always_on_top)
        .resizable(spec.resizable)
        .skip_taskbar(spec.skip_taskbar)
        .decorations(spec.decorations)
        // R-006: не забираем фокус — основное окно остаётся отзывчивым (старт/стоп).
        .focused(spec.focused)
        .build()
        .map_err(|e| format!("не удалось открыть окно оверлея: {e}"))?;
    Ok(())
}

/// Закрыть компакт-окно статуса (вызывается из UI основного окна и из самого
/// оверлея). Отсутствие окна — не ошибка (идемпотентно). `async` — по той же
/// причине, что и открытие (R-006): операции с окном из синхронной команды на
/// Windows/Linux рискуют заблокировать цикл событий.
#[tauri::command]
pub async fn close_compact_overlay(app: AppHandle) -> Result<(), String> {
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

    // R-006 (этап 13.6): чистая логика окна оверлея. Нативная сборка/рендер/фокус
    // на Windows — по ручному чек-листу (`docs/ui_adaptive.md`), в CI без дисплея
    // не исполняется. Здесь фиксируем контракт спецификации окна.
    #[test]
    fn overlay_spec_fixes_white_frame_and_focus_steal() {
        let spec = overlay_spec();
        // Фон окна — тёмный токен дизайн-системы (--dark #1f1c16), непрозрачный:
        // WebView2 не мигает белым до первого рендера.
        assert_eq!(spec.background, Color(0x1f, 0x1c, 0x16, 0xff));
        // Окно не забирает фокус на открытии → старт/стоп в основном окне живы.
        assert!(!spec.focused);
        // Поведение окна не деградирует: поверх всех, без рамки, вне таскбара.
        assert!(spec.always_on_top);
        assert!(!spec.decorations);
        assert!(spec.skip_taskbar);
        assert!(!spec.resizable);
        // Маршрут окна — тот же `index.html` с меткой `?window=overlay`.
        assert_eq!(spec.label, OVERLAY_LABEL);
        assert_eq!(spec.url, "index.html?window=overlay");
    }
}
