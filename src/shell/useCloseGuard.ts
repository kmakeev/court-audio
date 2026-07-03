import { useEffect, useRef, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';

// Защита идущей записи от случайного закрытия окна (этап 10.6, deliverable 4).
// Перехватываем `tauri://close-requested` (core-API `onCloseRequested`, без
// плагина): при активной записи отменяем закрытие и просим подтверждение. Само
// закрытие — `destroy()` после подтверждения. Актуальное состояние записи держим
// в ref, чтобы слушатель (регистрируется один раз) видел свежее значение.

export interface CloseGuard {
  /** Показывать ли диалог подтверждения закрытия. */
  pending: boolean;
  /** Подтвердить закрытие окна (запись при этом продолжается в ядре до стопа). */
  confirmClose: () => void;
  /** Отменить закрытие, остаться в приложении. */
  cancelClose: () => void;
}

export function useCloseGuard(recordingActive: boolean): CloseGuard {
  const activeRef = useRef(recordingActive);
  activeRef.current = recordingActive;
  const [pending, setPending] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let mounted = true;
    getCurrentWindow()
      .onCloseRequested((event: { preventDefault: () => void }) => {
        // Без активной записи — не мешаем штатному закрытию.
        if (!activeRef.current) return;
        event.preventDefault();
        setPending(true);
      })
      .then((u) => {
        if (mounted) unlisten = u;
        else u();
      })
      .catch(() => {
        // Нет нативного окна (например, тесты/веб-превью) — защита не нужна.
      });
    return () => {
      mounted = false;
      unlisten?.();
    };
  }, []);

  const confirmClose = () => {
    setPending(false);
    void getCurrentWindow()
      .destroy()
      .catch(() => {});
  };

  const cancelClose = () => setPending(false);

  return { pending, confirmClose, cancelClose };
}
