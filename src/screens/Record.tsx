import { useCallback, useEffect, useState } from 'react';
import type { Dispatch, SetStateAction } from 'react';
import { BlockHead, Button, Card, CriticalNotice, Field, Select, Tag } from '../design';
import {
  discardSession,
  listAudioDevices,
  onAudioLevel,
  onCaptureState,
  onReliabilityWarning,
  pauseCapture,
  recoverSession,
  resumeCapture,
  scanRecoverable,
  startCapture,
  stopCapture,
  type CaptureStateValue,
  type DeviceInfo,
  type LevelEvent,
  type RecoverableSession,
  type ReliabilityEvent,
} from '../lib/core';
import { getSettings, type Settings } from '../lib/settings';

// Экран «Запись» (этап 04). UI станции захвата: устройство, живые индикаторы
// уровня, управление сессией и недвусмысленный статус. Вся логика — в ядре
// (этапы 01–03); здесь только команды и отображение событий.

// ── Константы отображения (НЕ бизнес-параметры реестра) ──────────────────────
// Полная шкала нормированного PCM-уровня: клиппинг при пике у предела сигнала.
const FULL_SCALE = 1.0;
// Порог визуальной индикации клиппинга (доля от полной шкалы) — косметика метра.
const CLIP_RATIO = 0.99;
// Такт обновления хронометра — только отображение (раз в секунду).
const CLOCK_TICK_MS = 1000;

type UiState = 'idle' | CaptureStateValue;

const STATUS_LABEL: Record<UiState, string> = {
  idle: 'Готов к записи',
  recording: 'Идёт запись',
  paused: 'Пауза',
  stopping: 'Остановка…',
  stopped: 'Запись завершена',
};

const STATUS_TONE: Record<UiState, 'default' | 'accent' | 'gold' | 'green'> = {
  idle: 'default',
  recording: 'accent',
  paused: 'gold',
  stopping: 'default',
  stopped: 'green',
};

export function RecordScreen() {
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [deviceName, setDeviceName] = useState('');
  const [settings, setSettings] = useState<Settings | null>(null);
  const [state, setState] = useState<UiState>('idle');
  const [level, setLevel] = useState<LevelEvent>({ peak: 0, rms: 0 });
  const [elapsedSec, setElapsedSec] = useState(0);
  const [caseRef, setCaseRef] = useState('');
  const [error, setError] = useState<string | null>(null);

  // Активные предупреждения надёжности (по событиям ядра).
  const [deviceLost, setDeviceLost] = useState(false);
  const [diskWarning, setDiskWarning] = useState<ReliabilityEvent | null>(null);
  const [maxDuration, setMaxDuration] = useState(false);

  // Незавершённые сессии для восстановления (скан при монтировании).
  const [recoverable, setRecoverable] = useState<RecoverableSession[]>([]);

  const handleWarning = useCallback((e: ReliabilityEvent) => {
    switch (e.kind) {
      case 'device_lost':
        setDeviceLost(true);
        break;
      case 'device_back':
        setDeviceLost(false);
        break;
      case 'disk_low':
      case 'disk_critical':
        setDiskWarning(e);
        break;
      case 'max_duration_warning':
        setMaxDuration(true);
        break;
      case 'watchdog_restart':
        // Перезапуск прозрачен для оператора — запись продолжается.
        break;
    }
  }, []);

  // Первичная загрузка устройств/настроек/скана восстановления.
  useEffect(() => {
    let active = true;
    Promise.all([listAudioDevices(), getSettings(), scanRecoverable()])
      .then(([devs, s, rec]) => {
        if (!active) return;
        setDevices(devs);
        setSettings(s);
        setRecoverable(rec);
        const preferred =
          s.audio.device ?? devs.find((d) => d.is_default)?.name ?? devs[0]?.name ?? '';
        setDeviceName(preferred);
      })
      .catch((e: unknown) => active && setError(describeError(e)));
    return () => {
      active = false;
    };
  }, []);

  // Подписки на события ядра (уровень, состояние, надёжность).
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    let active = true;
    const wire = async () => {
      const a = await onAudioLevel(setLevel);
      const b = await onCaptureState((e) => setState(e.state));
      const c = await onReliabilityWarning(handleWarning);
      if (active) {
        unlisteners.push(a, b, c);
      } else {
        a();
        b();
        c();
      }
    };
    void wire();
    return () => {
      active = false;
      unlisteners.forEach((u) => u());
    };
  }, [handleWarning]);

  // Хронометр: тикаем раз в секунду, пока идёт запись.
  useEffect(() => {
    if (state !== 'recording') return;
    const id = window.setInterval(() => setElapsedSec((s) => s + 1), CLOCK_TICK_MS);
    return () => window.clearInterval(id);
  }, [state]);

  const onStart = useCallback(async () => {
    setError(null);
    setElapsedSec(0);
    setDeviceLost(false);
    setDiskWarning(null);
    setMaxDuration(false);
    try {
      await startCapture();
      setState('recording');
    } catch (e) {
      setError(describeError(e));
    }
  }, []);

  const onStop = useCallback(async () => {
    try {
      await stopCapture();
      setState('stopped');
      setLevel({ peak: 0, rms: 0 });
    } catch (e) {
      setError(describeError(e));
    }
  }, []);

  const onPause = useCallback(async () => {
    try {
      await pauseCapture();
      setState('paused');
    } catch (e) {
      setError(describeError(e));
    }
  }, []);

  const onResume = useCallback(async () => {
    try {
      await resumeCapture();
      setState('recording');
    } catch (e) {
      setError(describeError(e));
    }
  }, []);

  // Горячие клавиши: R — старт, S — стоп, Пробел — пауза/возобновление.
  // Не перехватываем, когда фокус в поле ввода (привязка к делу).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) return;
      const k = e.key.toLowerCase();
      if (k === 'r' && (state === 'idle' || state === 'stopped')) {
        e.preventDefault();
        void onStart();
      } else if (k === 's' && (state === 'recording' || state === 'paused')) {
        e.preventDefault();
        void onStop();
      } else if (e.key === ' ' && state === 'recording') {
        e.preventDefault();
        void onPause();
      } else if (e.key === ' ' && state === 'paused') {
        e.preventDefault();
        void onResume();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [state, onStart, onStop, onPause, onResume]);

  const isActive = state === 'recording' || state === 'paused';
  const quality = settings
    ? `${formatHz(settings.audio.sample_rate_hz)} · ${settings.audio.bit_depth} бит · ${formatChannels(settings.audio.channels)}`
    : '—';

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
      <Card>
        <BlockHead
          numeral="01"
          title="Запись заседания"
          hint="Захват звука, привязка к делу и выгрузка в экспертную систему"
        />
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginTop: 4 }}>
          <Tag
            tone={STATUS_TONE[state]}
            role="status"
            aria-live="polite"
            style={{ fontSize: 13, padding: '4px 12px' }}
          >
            {isRecordingDot(state)}
            {STATUS_LABEL[state]}
          </Tag>
          <span
            className="num"
            aria-label="Хронометраж записи"
            style={{ fontSize: 22, fontVariantNumeric: 'tabular-nums', color: 'var(--ink)' }}
          >
            {formatClock(elapsedSec)}
          </span>
        </div>
      </Card>

      {error && (
        <CriticalNotice variant="critical" title="Ошибка записи" description={error} />
      )}

      {/* Баннер восстановления — решение из этапа 02 (продолжить/закрыть). */}
      {recoverable.length > 0 && (
        <Card variant="accent">
          <BlockHead
            numeral="!"
            title="Найдена незавершённая сессия"
            hint="После сбоя осталась запись. Продолжить её восстановление или закрыть."
          />
          {recoverable.map((r) => (
            <div
              key={r.dir}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 12,
                marginTop: 8,
                flexWrap: 'wrap',
              }}
            >
              <span className="num" style={{ fontSize: 12, color: 'var(--ink-soft)', flex: 1 }}>
                {r.dir} · сегментов: {r.completed_segments}
                {r.already_recovered ? ' · уже восстановлена' : ''}
              </span>
              <Button
                variant="primary"
                onClick={() => void runRecover(r, recoverSession, setRecoverable, setError)}
              >
                Продолжить
              </Button>
              <Button
                variant="secondary"
                onClick={() => void runRecover(r, discardSession, setRecoverable, setError)}
              >
                Закрыть
              </Button>
            </div>
          ))}
        </Card>
      )}

      {/* Баннеры надёжности по событиям ядра. */}
      {deviceLost && (
        <CriticalNotice
          variant="critical"
          title="Устройство ввода пропало"
          description="Запись на паузе. При возврате устройства запись возобновится автоматически (если включено авто-возобновление)."
        />
      )}
      {diskWarning && (diskWarning.kind === 'disk_low' || diskWarning.kind === 'disk_critical') && (
        <CriticalNotice
          variant={diskWarning.kind === 'disk_critical' ? 'critical' : 'warning'}
          title={
            diskWarning.kind === 'disk_critical'
              ? 'Критически мало места на диске'
              : 'Мало свободного места на диске'
          }
          description={`Свободно ${diskWarning.free_mb} МБ. ${
            diskWarning.kind === 'disk_critical'
              ? 'Выполнен защитный стоп для сохранения записанного.'
              : 'Освободите место, чтобы запись не прервалась.'
          }`}
        />
      )}
      {maxDuration && (
        <CriticalNotice
          variant="warning"
          title="Достигнута предельная длительность сессии"
          description="Запись продолжается. Рекомендуется завершить и начать новую сессию."
        />
      )}

      <Card>
        <BlockHead numeral="A" title="Источник звука" />
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(260px, 1fr))',
            gap: 16,
            marginTop: 12,
          }}
        >
          <label style={{ display: 'block' }}>
            <span style={fieldLabelStyle}>Устройство ввода</span>
            <Select
              ariaLabel="Устройство ввода"
              value={deviceName}
              onChange={setDeviceName}
              disabled={isActive}
              options={devices.map((d) => ({
                value: d.name,
                label: d.is_default ? `${d.name} (по умолчанию)` : d.name,
              }))}
              placeholder={devices.length ? '— выберите устройство —' : 'Устройства не найдены'}
            />
          </label>
          <div>
            <span style={fieldLabelStyle}>Качество записи</span>
            <Tag tone="default" style={{ marginTop: 2 }}>
              {quality}
            </Tag>
          </div>
        </div>

        <div style={{ marginTop: 20 }}>
          <span style={fieldLabelStyle}>Уровень сигнала</span>
          <LevelMeter level={level} active={state === 'recording'} />
        </div>
      </Card>

      <Card>
        <BlockHead
          numeral="B"
          title="Привязка к делу"
          hint="№ дела / ФИО — пока ручной ввод (кэш дел подключится на этапе 05)"
        />
        <div style={{ marginTop: 12, maxWidth: 420 }}>
          <Field
            label="Дело / стороны"
            placeholder="напр. № 1-123/2026, Иванов И.И."
            value={caseRef}
            onChange={(e) => setCaseRef(e.target.value)}
          />
        </div>
      </Card>

      <div style={{ display: 'flex', gap: 12, flexWrap: 'wrap' }}>
        {!isActive && (
          <Button variant="primary" onClick={() => void onStart()} disabled={!deviceName}>
            ● Старт записи
          </Button>
        )}
        {state === 'recording' && (
          <Button variant="secondary" onClick={() => void onPause()}>
            ❚❚ Пауза
          </Button>
        )}
        {state === 'paused' && (
          <Button variant="secondary" onClick={() => void onResume()}>
            ▶ Возобновить
          </Button>
        )}
        {isActive && (
          <Button variant="primary" onClick={() => void onStop()}>
            ■ Стоп
          </Button>
        )}
      </div>

      <p style={{ fontSize: 12, color: 'var(--muted)', margin: 0 }}>
        Горячие клавиши: <kbd>R</kbd> — старт, <kbd>S</kbd> — стоп,{' '}
        <kbd>Пробел</kbd> — пауза/возобновление.
      </p>
    </div>
  );
}

// ── Индикатор уровня: столбики RMS/пик + клиппинг (решение «Решено») ─────────
function LevelMeter({ level, active }: { level: LevelEvent; active: boolean }) {
  const rmsPct = Math.min(level.rms / FULL_SCALE, 1) * 100;
  const peakPct = Math.min(level.peak / FULL_SCALE, 1) * 100;
  const clipping = active && level.peak >= FULL_SCALE * CLIP_RATIO;
  return (
    <div
      role="meter"
      aria-label="Индикатор уровня сигнала"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(rmsPct)}
      style={{ marginTop: 8 }}
    >
      <div
        style={{
          position: 'relative',
          height: 22,
          background: 'var(--paper-strong)',
          border: '1px solid var(--hairline)',
          overflow: 'hidden',
        }}
      >
        {/* Заливка RMS */}
        <div
          style={{
            position: 'absolute',
            inset: 0,
            width: `${rmsPct}%`,
            background: clipping ? 'var(--accent)' : 'var(--green)',
            transition: 'width 80ms linear',
          }}
        />
        {/* Маркер пика */}
        <div
          aria-hidden="true"
          style={{
            position: 'absolute',
            top: 0,
            bottom: 0,
            left: `calc(${peakPct}% - 1px)`,
            width: 2,
            background: 'var(--ink)',
          }}
        />
      </div>
      <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: 4 }}>
        <span className="num" style={{ fontSize: 11, color: 'var(--muted)' }}>
          RMS {Math.round(rmsPct)}% · пик {Math.round(peakPct)}%
        </span>
        {clipping && <Tag tone="accent">Клиппинг</Tag>}
      </div>
    </div>
  );
}

const fieldLabelStyle = {
  display: 'block',
  fontSize: 11,
  textTransform: 'uppercase' as const,
  letterSpacing: '0.14em',
  color: 'var(--muted)',
  marginBottom: 8,
  fontWeight: 500,
};

function isRecordingDot(state: UiState) {
  if (state !== 'recording') return null;
  return (
    <span
      aria-hidden="true"
      style={{
        display: 'inline-block',
        width: 8,
        height: 8,
        borderRadius: '50%',
        background: 'var(--accent)',
      }}
    />
  );
}

async function runRecover(
  r: RecoverableSession,
  action: (dir: string) => Promise<void>,
  setRecoverable: Dispatch<SetStateAction<RecoverableSession[]>>,
  setError: (e: string | null) => void,
) {
  try {
    await action(r.dir);
    setRecoverable((prev) => prev.filter((x) => x.dir !== r.dir));
  } catch (e) {
    setError(describeError(e));
  }
}

function formatClock(totalSec: number): string {
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(h)}:${pad(m)}:${pad(s)}`;
}

function formatHz(hz: number): string {
  return `${(hz / 1000).toFixed(1).replace(/\.0$/, '')} кГц`;
}

function formatChannels(n: number): string {
  if (n === 1) return 'моно';
  if (n === 2) return 'стерео';
  return `${n} кан.`;
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}
