import { Navigate, Route, Routes } from 'react-router-dom';
import { AppShell } from './shell/AppShell';
import { RecordScreen } from './screens/Record';
import { SessionsScreen } from './screens/Sessions';
import { SettingsScreen } from './screens/Settings';
import { DiagnosticsScreen } from './screens/Diagnostics';

// Маршрутизация четырёх экранов-заглушек этапа 00. Реальная функциональность
// наполняется на этапах 04 (UI записи) и далее.
export function App() {
  return (
    <Routes>
      <Route element={<AppShell />}>
        <Route index element={<RecordScreen />} />
        <Route path="sessions" element={<SessionsScreen />} />
        <Route path="settings" element={<SettingsScreen />} />
        <Route path="diagnostics" element={<DiagnosticsScreen />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Route>
    </Routes>
  );
}
