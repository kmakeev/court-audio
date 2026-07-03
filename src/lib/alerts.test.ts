import { describe, expect, it } from 'vitest';
import { reliabilityToAlert, shouldBeep } from './alerts';
import type { ReliabilityEvent } from './core';

// Чистая логика звукового сигнала (этап 10.6): бипаем только при включённой опции
// и только на тревожные события.

describe('shouldBeep', () => {
  it('не бипает при выключенной опции', () => {
    expect(shouldBeep(false, { kind: 'device_lost' })).toBe(false);
    expect(shouldBeep(false, { kind: 'disk_critical' })).toBe(false);
    expect(shouldBeep(false, { kind: 'upload_failed' })).toBe(false);
  });

  it('бипает на тревожные события при включённой опции', () => {
    expect(shouldBeep(true, { kind: 'device_lost' })).toBe(true);
    expect(shouldBeep(true, { kind: 'disk_critical' })).toBe(true);
    expect(shouldBeep(true, { kind: 'upload_failed' })).toBe(true);
  });
});

describe('reliabilityToAlert', () => {
  const cases: [ReliabilityEvent, boolean][] = [
    [{ kind: 'device_lost' }, true],
    [{ kind: 'disk_critical', free_mb: 10 }, true],
    [{ kind: 'device_back' }, false],
    [{ kind: 'disk_low', free_mb: 500 }, false],
    [{ kind: 'watchdog_restart' }, false],
    [{ kind: 'max_duration_warning' }, false],
  ];

  it('тревожит только на обрыв устройства и критический диск', () => {
    for (const [event, expected] of cases) {
      expect(reliabilityToAlert(event) !== null).toBe(expected);
    }
  });
});
