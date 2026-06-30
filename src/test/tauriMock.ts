// Управляемый мок Tauri-моста для компонентных тестов (этап 04). Эмулирует
// `invoke` (команды) и `listen` (события) ядра без реального Tauri-рантайма.
// Тесты регистрируют обработчики команд (`setInvoke`) и проталкивают события
// (`emitEvent`).

type Handler = (args?: unknown) => unknown;

const handlers = new Map<string, Handler>();
const listeners = new Map<string, Set<(e: { payload: unknown }) => void>>();

/** Зарегистрировать обработчик команды (значение или throw для ошибки). */
export function setInvoke(cmd: string, handler: Handler): void {
  handlers.set(cmd, handler);
}

/** Сбросить все обработчики и подписки (между тестами). */
export function resetTauriMock(): void {
  handlers.clear();
  listeners.clear();
}

/** Реализация `invoke`: делегирует зарегистрированному обработчику. */
export function mockInvoke(cmd: string, args?: unknown): Promise<unknown> {
  const handler = handlers.get(cmd);
  if (!handler) {
    return Promise.reject(new Error(`нет мок-обработчика команды: ${cmd}`));
  }
  try {
    return Promise.resolve(handler(args));
  } catch (e) {
    return Promise.reject(e);
  }
}

/** Реализация `listen`: регистрирует подписчика, возвращает unlisten. */
export function mockListen(
  name: string,
  cb: (e: { payload: unknown }) => void,
): Promise<() => void> {
  let set = listeners.get(name);
  if (!set) {
    set = new Set();
    listeners.set(name, set);
  }
  set.add(cb);
  return Promise.resolve(() => {
    set?.delete(cb);
  });
}

/** Протолкнуть событие всем текущим подписчикам. */
export function emitEvent(name: string, payload: unknown): void {
  listeners.get(name)?.forEach((cb) => cb({ payload }));
}
