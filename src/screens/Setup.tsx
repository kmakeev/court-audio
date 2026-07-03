import { useCallback, useEffect, useState } from 'react';
import type { CSSProperties } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import {
  BlockHead,
  Button,
  Card,
  Field,
  NEUTRAL_BTN,
  screenStackStyle,
  Select,
  Tag,
} from '../design';
import { SelfTestPanel } from '../components/SelfTest';
import {
  listAudioDevices,
  onAudioLevel,
  startCapture,
  startMonitor,
  stopCapture,
  stopMonitor,
  type DeviceInfo,
  type LevelEvent,
} from '../lib/core';
import { aggregateLevel, toMeterPct } from '../lib/format';
import { getSettings, saveSettings, type Settings } from '../lib/settings';

// Мастер первого запуска новой станции (этап 10.6, deliverable 2). Проводит
// оператора/администратора по минимуму настройки без чтения доков: адрес сервера,
// выбор и проверка устройства (живой монитор уровня), тестовая запись с
// прослушиванием, финальная self-test-проверка. Повторно доступен с «Диагностики».
// Использует уже существующие команды ядра — новой логики нет.

// Кнопки одного ряда мастера должны быть одной высоты (DS: primary=44,
// secondary=38). Приводим обе к явной высоте 44 (+ ряд выравниваем по центру),
// чтобы «Назад» и «Далее/Сохранить» стояли ровно.
const WIZARD_H = 44;
const WIZARD_PRIMARY: CSSProperties = { height: WIZARD_H };
const WIZARD_SECONDARY: CSSProperties = { ...NEUTRAL_BTN, height: WIZARD_H };

type StepId = 'server' | 'device' | 'test-record' | 'check';

const STEPS: { id: StepId; title: string }[] = [
  { id: 'server', title: 'Адрес сервера' },
  { id: 'device', title: 'Устройство записи' },
  { id: 'test-record', title: 'Тестовая запись' },
  { id: 'check', title: 'Проверка готовности' },
];

export function SetupScreen() {
  const navigate = useNavigate();
  const location = useLocation();
  // Возврат из проигрывателя (шаг «тестовая запись») восстанавливает нужный шаг —
  // иначе мастер начинался бы заново после прослушивания тестовой записи.
  const initialStep =
    location.state && typeof location.state === 'object' && 'step' in location.state
      ? Number((location.state as { step: number }).step) || 0
      : 0;
  const [step, setStep] = useState(initialStep);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getSettings().then(setSettings).catch((e) => setError(describeError(e)));
    listAudioDevices().then(setDevices).catch(() => setDevices([]));
  }, []);

  const current = STEPS[step];

  return (
    <div style={screenStackStyle(760)}>
      <Card>
        <div style={{ display: 'flex', alignItems: 'flex-start', gap: 12, flexWrap: 'wrap' }}>
          <div style={{ flex: 1, minWidth: 220 }}>
            <BlockHead
              numeral="★"
              title="Мастер первого запуска"
              hint="Доведёт станцию до рабочего состояния без чтения документации"
            />
          </div>
          <Button variant="secondary" style={WIZARD_SECONDARY} onClick={() => navigate('/diagnostics')}>
            Закрыть мастер
          </Button>
        </div>
        <StepBar step={step} />
        {error && (
          <div style={{ marginTop: 8 }}>
            <Tag tone="accent">Ошибка: {error}</Tag>
          </div>
        )}
      </Card>

      {settings && current.id === 'server' && (
        <ServerStep
          settings={settings}
          onSettings={setSettings}
          onError={setError}
          onNext={() => setStep(1)}
        />
      )}
      {settings && current.id === 'device' && (
        <DeviceStep
          settings={settings}
          devices={devices}
          onSettings={setSettings}
          onError={setError}
          onBack={() => setStep(0)}
          onNext={() => setStep(2)}
        />
      )}
      {current.id === 'test-record' && (
        <TestRecordStep onError={setError} onBack={() => setStep(1)} onNext={() => setStep(3)} />
      )}
      {current.id === 'check' && (
        <>
          <SelfTestPanel numeral="✓" />
          <div style={{ display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap' }}>
            <Button variant="secondary" style={WIZARD_SECONDARY} onClick={() => setStep(2)}>
              ← Назад
            </Button>
            <Button variant="primary" style={WIZARD_PRIMARY} onClick={() => navigate('/')}>
              Завершить и перейти к записи
            </Button>
          </div>
        </>
      )}
    </div>
  );
}

function StepBar({ step }: { step: number }) {
  return (
    <div style={{ display: 'flex', gap: 8, marginTop: 12, flexWrap: 'wrap' }}>
      {STEPS.map((s, i) => (
        <Tag key={s.id} tone={i === step ? 'accent' : i < step ? 'green' : 'default'}>
          {i + 1}. {s.title}
        </Tag>
      ))}
    </div>
  );
}

function ServerStep({
  settings,
  onSettings,
  onError,
  onNext,
}: {
  settings: Settings;
  onSettings: (s: Settings) => void;
  onError: (e: string | null) => void;
  onNext: () => void;
}) {
  const [url, setUrl] = useState(settings.sync.server_base_url ?? '');
  const [saving, setSaving] = useState(false);

  async function saveAndNext() {
    setSaving(true);
    onError(null);
    try {
      const next = structuredClone(settings);
      next.sync.server_base_url = url.trim() === '' ? null : url.trim();
      // Мастер первого запуска задаёт базовый URL; серверные/опасные подтверждения
      // — гейт ядра (10.4). Пустой URL допустим (оффлайн-зал настроят позже).
      const outcome = await saveSettings(next, true);
      if (outcome.kind === 'saved') {
        onSettings(next);
        onNext();
      } else {
        onError('Изменение требует прав администратора (разблокируйте админ-доступ).');
      }
    } catch (e) {
      onError(describeError(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card>
      <BlockHead numeral="1" title="Адрес сервера ex_system" hint="Куда станция выгружает записи и куда входит оператор" />
      <div style={{ marginTop: 12 }}>
        <Field
          label="URL сервера"
          placeholder="https://…"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
        />
      </div>
      <p style={{ fontSize: 12, color: 'var(--muted)', margin: '10px 0 0' }}>
        Можно оставить пустым для оффлайн-зала — задать позже в «Администрировании».
      </p>
      <div style={{ marginTop: 16 }}>
        <Button variant="primary" style={WIZARD_PRIMARY} loading={saving} onClick={() => void saveAndNext()}>
          Сохранить и далее →
        </Button>
      </div>
    </Card>
  );
}

function DeviceStep({
  settings,
  devices,
  onSettings,
  onError,
  onBack,
  onNext,
}: {
  settings: Settings;
  devices: DeviceInfo[];
  onSettings: (s: Settings) => void;
  onError: (e: string | null) => void;
  onBack: () => void;
  onNext: () => void;
}) {
  const [device, setDevice] = useState(settings.audio.device ?? '');
  const [saving, setSaving] = useState(false);

  // Живой монитор уровня — подтверждает, что микрофон работает (открывает устройство).
  useEffect(() => {
    void startMonitor().catch(() => {});
    return () => {
      void stopMonitor().catch(() => {});
    };
  }, []);

  async function saveAndNext() {
    setSaving(true);
    onError(null);
    try {
      const next = structuredClone(settings);
      next.audio.device = device.trim() === '' ? null : device;
      const outcome = await saveSettings(next, true);
      if (outcome.kind === 'saved') {
        onSettings(next);
        onNext();
      } else {
        onError('Изменение требует прав администратора.');
      }
    } catch (e) {
      onError(describeError(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card>
      <BlockHead numeral="2" title="Устройство записи" hint="Выберите микрофон зала и убедитесь, что уровень «живой»" />
      <div style={{ marginTop: 12 }}>
        <Select
          ariaLabel="Устройство ввода"
          value={device}
          onChange={setDevice}
          options={devices.map((d) => ({
            value: d.name,
            label: d.is_default ? `${d.name} (по умолчанию)` : d.name,
          }))}
          placeholder={devices.length ? '— системное по умолчанию —' : 'Устройства не найдены'}
        />
      </div>
      <p style={{ fontSize: 12, color: 'var(--muted)', margin: '10px 0 0' }}>
        Скажите что-нибудь в микрофон — индикатор уровня на экране «Запись» должен
        реагировать. Здесь монитор уже запущен (доступ к микрофону мог быть запрошен).
      </p>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 16, flexWrap: 'wrap' }}>
        <Button variant="secondary" style={WIZARD_SECONDARY} onClick={onBack}>
          ← Назад
        </Button>
        <Button variant="primary" style={WIZARD_PRIMARY} loading={saving} onClick={() => void saveAndNext()}>
          Сохранить и далее →
        </Button>
      </div>
    </Card>
  );
}

function TestRecordStep({
  onError,
  onBack,
  onNext,
}: {
  onError: (e: string | null) => void;
  onBack: () => void;
  onNext: () => void;
}) {
  const [phase, setPhase] = useState<'idle' | 'recording' | 'done'>('idle');
  const [dir, setDir] = useState<string | null>(null);
  const navigate = useNavigate();

  // Живой индикатор уровня во время тестовой записи (как на экране «Запись»):
  // подтверждает, что звук реально идёт в запись. Захват сам эмитит `audio_level`.
  const [levels, setLevels] = useState<Record<number, LevelEvent>>({});
  useEffect(() => {
    if (phase !== 'recording') return;
    let active = true;
    let unlisten: (() => void) | undefined;
    onAudioLevel((e) => setLevels((prev) => ({ ...prev, [e.track_id ?? 0]: e }))).then((u) => {
      if (active) unlisten = u;
      else u();
    });
    return () => {
      active = false;
      unlisten?.();
      setLevels({});
    };
  }, [phase]);

  const start = useCallback(async () => {
    onError(null);
    try {
      const started = await startCapture();
      setDir(started.output_dir);
      setPhase('recording');
    } catch (e) {
      onError(describeError(e));
    }
  }, [onError]);

  const stop = useCallback(async () => {
    try {
      await stopCapture();
      setPhase('done');
    } catch (e) {
      onError(describeError(e));
      setPhase('done');
    }
  }, [onError]);

  return (
    <Card>
      <BlockHead numeral="3" title="Тестовая запись" hint="Короткая запись с последующим прослушиванием — проверка сквозного пути" />
      <div style={{ marginTop: 12, display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap' }}>
        {phase === 'idle' && (
          <Button variant="primary" style={WIZARD_PRIMARY} onClick={() => void start()}>
            ● Начать тестовую запись
          </Button>
        )}
        {phase === 'recording' && (
          <Button variant="primary" style={WIZARD_PRIMARY} onClick={() => void stop()}>
            ■ Остановить
          </Button>
        )}
        {phase === 'done' && dir && (
          <Button
            variant="secondary"
            style={WIZARD_SECONDARY}
            onClick={() =>
              navigate(`/sessions/${encodeURIComponent(dir)}/listen`, {
                // Проигрыватель по этому состоянию вернёт «← К мастеру» на шаг записи.
                state: { returnTo: '/setup', returnStep: 2 },
              })
            }
          >
            Прослушать тестовую запись
          </Button>
        )}
      </div>
      {phase === 'recording' && (
        <>
          <p style={{ fontSize: 13, color: 'var(--accent-deep)', margin: '12px 0 0' }} role="status">
            Идёт тестовая запись — скажите пару фраз и остановите.
          </p>
          <div style={{ marginTop: 12 }}>
            <span style={{ fontSize: 11, color: 'var(--muted)', textTransform: 'uppercase', letterSpacing: '0.12em', fontWeight: 500 }}>
              Уровень сигнала
            </span>
            <SetupLevelBar levels={levels} />
          </div>
        </>
      )}
      {phase === 'done' && (
        <p style={{ fontSize: 12, color: 'var(--muted)', margin: '12px 0 0' }}>
          Запись сохранена. Прослушайте её, затем перейдите к финальной проверке.
        </p>
      )}
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 16, flexWrap: 'wrap' }}>
        <Button variant="secondary" style={WIZARD_SECONDARY} onClick={onBack}>
          ← Назад
        </Button>
        <Button variant="primary" style={WIZARD_PRIMARY} disabled={phase === 'recording'} onClick={onNext}>
          Далее →
        </Button>
      </div>
    </Card>
  );
}

// Простой сводный индикатор уровня (один столбик RMS/пик) — как в «режиме зала»/
// оверлее (`aggregateLevel`); полноценные пофайловые метры не нужны для проверки.
function SetupLevelBar({ levels }: { levels: Record<number, LevelEvent> }) {
  const { rms, peak } = aggregateLevel(levels);
  const rmsPct = toMeterPct(rms);
  const peakPct = toMeterPct(peak);
  return (
    <div
      role="meter"
      aria-label="Индикатор уровня сигнала"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(rmsPct)}
      style={{
        position: 'relative',
        height: 22,
        marginTop: 6,
        background: 'var(--paper-strong)',
        border: '1px solid var(--hairline)',
        overflow: 'hidden',
      }}
    >
      <div
        className="level-meter-fill"
        style={{ position: 'absolute', inset: 0, width: `${rmsPct}%`, background: 'var(--green)' }}
      />
      <div
        aria-hidden="true"
        style={{ position: 'absolute', top: 0, bottom: 0, left: `calc(${peakPct}% - 1px)`, width: 2, background: 'var(--ink)' }}
      />
    </div>
  );
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}
