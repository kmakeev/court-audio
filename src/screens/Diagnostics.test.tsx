import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { DiagnosticsScreen } from './Diagnostics';
import { setInvoke } from '../test/tauriMock';
import { diagnosticsFixture } from '../test/fixtures';

function renderDiagnostics() {
  return render(
    <MemoryRouter>
      <DiagnosticsScreen />
    </MemoryRouter>,
  );
}

describe('DiagnosticsScreen', () => {
  it('показывает устройство, свободное место, события и целостность', async () => {
    setInvoke('diagnostics', () => diagnosticsFixture());
    renderDiagnostics();

    expect(await screen.findByText('Микрофон зала')).toBeInTheDocument();
    expect(screen.getByText('Достаточно')).toBeInTheDocument();
    // Целостность последней сессии.
    expect(screen.getByText('session-1700000000000')).toBeInTheDocument();
    expect(screen.getByText('все хешированы')).toBeInTheDocument();
    // Событие записи.
    expect(screen.getByText('Старт сессии')).toBeInTheDocument();
    // Версия станции.
    expect(screen.getByText('0.1.0')).toBeInTheDocument();
  });

  it('сообщает об ошибке команды диагностики', async () => {
    setInvoke('diagnostics', () => {
      throw new Error('диск недоступен');
    });
    renderDiagnostics();

    expect(await screen.findByText(/диск недоступен/)).toBeInTheDocument();
  });
});
