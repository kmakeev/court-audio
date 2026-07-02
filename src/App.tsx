import { Navigate, Route, Routes } from 'react-router-dom';
import { AppShell } from './shell/AppShell';
import { RecordScreen } from './screens/Record';
import { SessionsScreen } from './screens/Sessions';
import { PlaybackScreen } from './screens/Playback';
import { ExportScreen } from './screens/Export';
import { SettingsScreen } from './screens/Settings';
import { DiagnosticsScreen } from './screens/Diagnostics';

// Маршрутизация экранов. `sessions/:dir/listen` (этап 10.1) и
// `sessions/:dir/export` (этап 10.2) — вне бокового меню AppShell: точки
// входа только из карточки сессии в списке; полноценная карточка сессии —
// этап 10.6.
export function App() {
  return (
    <Routes>
      <Route element={<AppShell />}>
        <Route index element={<RecordScreen />} />
        <Route path="sessions" element={<SessionsScreen />} />
        <Route path="sessions/:dir/listen" element={<PlaybackScreen />} />
        <Route path="sessions/:dir/export" element={<ExportScreen />} />
        <Route path="settings" element={<SettingsScreen />} />
        <Route path="diagnostics" element={<DiagnosticsScreen />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Route>
    </Routes>
  );
}
