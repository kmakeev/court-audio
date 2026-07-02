import { describe, it, expect } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { RecordScreen } from './Record';
import { AuthProvider } from '../lib/auth-context';
import { setInvoke } from '../test/tauriMock';
import { settingsFixture, authStatusFixture } from '../test/fixtures';

// Гейт входа на экране записи (этап 10.3): без вошедшего оператора кнопка старта
// заблокирована (backend тоже отклонит старт).

function wireRecordDefaults() {
  setInvoke('list_audio_devices', () => [
    { name: 'Микрофон зала', is_default: true, default_sample_rate_hz: 44100, default_channels: 1, configs: [] },
  ]);
  setInvoke('get_settings', () => settingsFixture());
  setInvoke('scan_recoverable', () => []);
  setInvoke('capture_status', () => ({ state: 'idle', started_at_unix_ms: null, output_dir: null, segment_count: 0 }));
  setInvoke('start_monitor', () => undefined);
  setInvoke('stop_monitor', () => undefined);
  setInvoke('get_case_cache_status', () => ({ synced_at_unix_ms: null, is_fresh: false, record_count: 0, scope: 'court_docket' }));
  setInvoke('search_cases', () => []);
  setInvoke('list_annotations', () => ({ markers: [], role_spans: [] }));
}

function renderRecordWithAuth() {
  return render(
    <MemoryRouter>
      <AuthProvider>
        <RecordScreen />
      </AuthProvider>
    </MemoryRouter>,
  );
}

describe('RecordScreen — гейт входа', () => {
  it('блокирует старт без вошедшего оператора', async () => {
    wireRecordDefaults();
    setInvoke('auth_status', () => authStatusFixture({ operator: null }));

    renderRecordWithAuth();
    await screen.findByText('Готов к записи');

    const start = await screen.findByRole('button', { name: /Старт записи/ });
    await waitFor(() => expect(start).toBeDisabled());
  });

  it('разрешает старт вошедшему оператору', async () => {
    wireRecordDefaults();
    setInvoke('auth_status', () => authStatusFixture());

    renderRecordWithAuth();
    await screen.findByText('Готов к записи');

    const start = await screen.findByRole('button', { name: /Старт записи/ });
    await waitFor(() => expect(start).toBeEnabled());
  });
});
