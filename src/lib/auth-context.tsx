// Единый источник состояния входа оператора для UI (этап 10.3): шапка, гейт
// старта и экран входа читают один статус. Провайдер грузит `authStatus` при
// монтировании и подписан на событие `auth_state` (вход/выход/тихий refresh).
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from 'react';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { authStatus, onAuthState, type AuthStatus } from './core';

export interface AuthContextValue {
  /** Текущий статус входа (`null`, пока не загружен). */
  status: AuthStatus | null;
  /** Завершилась ли первичная загрузка статуса. */
  ready: boolean;
  /** Перечитать статус (после действий вне провайдера). */
  refresh: () => void;
}

// Пермиссивный дефолт — чтобы `useAuth` не бросал вне провайдера (изолированные
// тесты экранов без AuthProvider видят «статус неизвестен», а не ошибку).
const DEFAULT: AuthContextValue = {
  status: null,
  ready: true,
  refresh: () => {},
};

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<AuthStatus | null>(null);
  const [ready, setReady] = useState(false);

  const refresh = useCallback(() => {
    authStatus()
      .then((s) => {
        setStatus(s);
        setReady(true);
      })
      .catch(() => setReady(true));
  }, []);

  useEffect(() => {
    let active = true;
    let unlisten: UnlistenFn | undefined;
    authStatus()
      .then((s) => {
        if (active) {
          setStatus(s);
          setReady(true);
        }
      })
      .catch(() => {
        if (active) setReady(true);
      });
    onAuthState((s) => {
      if (active) setStatus(s);
    }).then((u) => {
      if (active) unlisten = u;
      else u();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  return (
    <AuthContext.Provider value={{ status, ready, refresh }}>
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth(): AuthContextValue {
  return useContext(AuthContext) ?? DEFAULT;
}

/** Вошёл ли оператор (в онлайне или по оффлайн-разблокировке). */
export function isAuthenticated(status: AuthStatus | null): boolean {
  return !!status?.operator;
}
