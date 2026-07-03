import { describe, it, expect } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { setInvoke } from '../test/tauriMock';
import {
  isSessionActive,
  recordingStatusLabel,
  recordingStatusTone,
  RecordingStatusProvider,
  useRecordingStatus,
} from './recording-status';

describe('recording-status — чистые хелперы', () => {
  it('метки и тона покрывают все состояния', () => {
    expect(recordingStatusLabel('idle')).toBe('Готов к записи');
    expect(recordingStatusLabel('recording')).toBe('Идёт запись');
    expect(recordingStatusLabel('paused')).toBe('Пауза');
    expect(recordingStatusTone('recording')).toBe('accent');
    expect(recordingStatusTone('paused')).toBe('gold');
    expect(recordingStatusTone('idle')).toBe('default');
  });

  it('активной считается запись/пауза/остановка', () => {
    expect(isSessionActive('recording')).toBe(true);
    expect(isSessionActive('paused')).toBe(true);
    expect(isSessionActive('stopping')).toBe(true);
    expect(isSessionActive('idle')).toBe(false);
    expect(isSessionActive('stopped')).toBe(false);
  });
});

// Мелкий потребитель контекста для проверки провайдера.
function StatusProbe() {
  const { state, startedAtMs } = useRecordingStatus();
  return (
    <div>
      <span data-testid="state">{state}</span>
      <span data-testid="started">{String(startedAtMs != null)}</span>
    </div>
  );
}

describe('RecordingStatusProvider', () => {
  it('поднимает состояние из снимка capture_status (запись шла в фоне)', async () => {
    setInvoke('capture_status', () => ({
      state: 'recording',
      started_at_unix_ms: Date.now() - 5000,
      output_dir: '/data/rec',
      segment_count: 3,
    }));
    render(
      <RecordingStatusProvider>
        <StatusProbe />
      </RecordingStatusProvider>,
    );
    await waitFor(() => expect(screen.getByTestId('state')).toHaveTextContent('recording'));
    expect(screen.getByTestId('started')).toHaveTextContent('true');
  });

  it('в простое остаётся idle', async () => {
    setInvoke('capture_status', () => ({
      state: 'idle',
      started_at_unix_ms: null,
      output_dir: null,
      segment_count: 0,
    }));
    render(
      <RecordingStatusProvider>
        <StatusProbe />
      </RecordingStatusProvider>,
    );
    // Дать промису capture_status разрешиться.
    await waitFor(() => expect(screen.getByTestId('state')).toHaveTextContent('idle'));
    expect(screen.getByTestId('started')).toHaveTextContent('false');
  });
});
