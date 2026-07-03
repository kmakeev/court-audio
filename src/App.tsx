import { useEffect, useState } from 'react';
import { Navigate, Outlet, Route, Routes } from 'react-router-dom';
import { AppShell } from './shell/AppShell';
import { RecordScreen } from './screens/Record';
import { SessionsScreen } from './screens/Sessions';
import { SessionCardScreen } from './screens/SessionCard';
import { PlaybackScreen } from './screens/Playback';
import { ExportScreen } from './screens/Export';
import { SettingsScreen } from './screens/Settings';
import { AdministrationScreen } from './screens/Administration';
import { DiagnosticsScreen } from './screens/Diagnostics';
import { SetupScreen } from './screens/Setup';
import { LoginScreen } from './screens/Login';
import { AuthProvider, useAuth } from './lib/auth-context';
import { getSettings } from './lib/settings';

// Маршрутизация экранов. `sessions/:dir/listen` (этап 10.1) и
// `sessions/:dir/export` (этап 10.2) — вне бокового меню AppShell: точки
// входа только из карточки сессии в списке; полноценная карточка сессии —
// этап 10.6. Гейт входа оператора (этап 10.3) — `RequireOperator`.
export function App() {
  return (
    <AuthProvider>
      <Routes>
        <Route path="login" element={<LoginScreen />} />
        <Route element={<RequireOperator />}>
          <Route element={<AppShell />}>
            <Route index element={<RecordScreen />} />
            <Route path="sessions" element={<SessionsScreen />} />
            <Route path="sessions/:dir" element={<SessionCardScreen />} />
            <Route path="sessions/:dir/listen" element={<PlaybackScreen />} />
            <Route path="sessions/:dir/export" element={<ExportScreen />} />
            <Route path="settings" element={<SettingsScreen />} />
            <Route path="administration" element={<AdministrationScreen />} />
            <Route path="diagnostics" element={<DiagnosticsScreen />} />
            <Route path="setup" element={<SetupScreen />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Route>
        </Route>
      </Routes>
    </AuthProvider>
  );
}

// Гейт входа (этап 10.3): при `auth.operator.required_to_start` без вошедшего
// оператора перенаправляет на экран входа. Пока статус/настройка не загружены —
// ничего не рендерим (без мигания редиректом).
function RequireOperator() {
  const { status, ready } = useAuth();
  const [required, setRequired] = useState<boolean | null>(null);

  useEffect(() => {
    let active = true;
    getSettings()
      .then((s) => active && setRequired(s.auth.operator.required_to_start))
      .catch(() => active && setRequired(false));
    return () => {
      active = false;
    };
  }, []);

  if (!ready || required === null) return null;
  if (required && !status?.operator) return <Navigate to="/login" replace />;
  return <Outlet />;
}
