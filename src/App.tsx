import { Navigate, Route, Routes } from 'react-router-dom';
import { AppShell } from './shell/AppShell';
import { RecordScreen } from './screens/Record';
import { SessionsScreen } from './screens/Sessions';
import { PlaybackScreen } from './screens/Playback';
import { SettingsScreen } from './screens/Settings';
import { DiagnosticsScreen } from './screens/Diagnostics';

// Маршрутизация экранов. `sessions/:dir/listen` (этап 10.1) — вне бокового
// меню AppShell: точка входа только с кнопки «Прослушать» в списке сессий;
// полноценная карточка сессии — этап 10.6.
export function App() {
  return (
    <Routes>
      <Route element={<AppShell />}>
        <Route index element={<RecordScreen />} />
        <Route path="sessions" element={<SessionsScreen />} />
        <Route path="sessions/:dir/listen" element={<PlaybackScreen />} />
        <Route path="settings" element={<SettingsScreen />} />
        <Route path="diagnostics" element={<DiagnosticsScreen />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Route>
    </Routes>
  );
}
