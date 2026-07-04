// Определение окна по query-параметру (этап 10.5; R-006, этап 13.6).
//
// Компакт-оверлей — отдельное Tauri-окно, которое ядро открывает с URL
// `index.html?window=overlay`. Роутинг вынесен в чистую функцию, чтобы покрыть
// его юнит-тестом без загрузки самого приложения: на Windows дефект R-006
// проявлялся в т.ч. как «маршрут `?window=overlay` не отрисовывается отдельным
// окном» — тест фиксирует, что предикат детерминирован для любого query-хвоста.

/** Метка окна оверлея в query (совпадает с `OVERLAY_URL` в ядре). */
export const OVERLAY_WINDOW = 'overlay';

/**
 * Это окно — компакт-оверлей? Разбирает `?window=overlay` из строки поиска.
 * Принимает строку (`window.location.search`) — чистая, тестируемая.
 */
export function isOverlayWindow(search: string): boolean {
  return new URLSearchParams(search).get('window') === OVERLAY_WINDOW;
}
