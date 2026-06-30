import { useCallback, useEffect, useState } from 'react';
import type { CSSProperties, Dispatch, SetStateAction } from 'react';
import { BlockHead, Button, Card, CriticalNotice, ProgressBar, Select, Tag } from '../design';
import { CasePicker } from '../components/CasePicker';
import { ConfirmDialog } from '../shell/ConfirmDialog';
import {
  bindSessionCase,
  discardSession,
  getCaptureStatus,
  listAudioDevices,
  onAudioLevel,
  onCaptureState,
  onReliabilityWarning,
  pauseCapture,
  recoverSession,
  resumeCapture,
  scanRecoverable,
  startCapture,
  startMonitor,
  stopCapture,
  stopMonitor,
  type AdjudicationRef,
  type CaptureStateValue,
  type ChannelLevel,
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
// Нижняя граница шкалы метра в дБFS: уровень меряем логарифмически (как все
// аудио-метры), иначе тихая речь в линейной шкале выглядит «мёртвой». Косметика.
const METER_FLOOR_DBFS = -60;
// Такт обновления хронометра — только отображение (раз в секунду).
const CLOCK_TICK_MS = 1000;
// Период опроса состояния захвата (живой счётчик сегментов) — отображение.
const STATUS_POLL_MS = 3000;
// Дебаунс записи изменённой привязки к делу в манифест во время записи — чтобы
// ввод не порождал команду на каждый символ. Косметика взаимодействия.
const BIND_DEBOUNCE_MS = 500;
// Минимальное время показа индикатора «Сохранение записи…»: короткие записи
// финализируются мгновенно, и без этого порога подтверждение мелькнуло бы
// незаметно. Только отображение, не бизнес-параметр реестра.
const STOP_INDICATOR_MIN_MS = 700;

// Читаемая нейтральная кнопка на светлом фоне: вариант `secondary` из DS
// рассчитан на тёмную шапку (светлый текст), поэтому переопределяем токенами.
const NEUTRAL_BTN: CSSProperties = { color: 'var(--ink)', borderColor: 'var(--ink-soft)' };

interface SessionInfo {
  output_dir: string | null;
  segment_count: number;
}

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
  const [level, setLevel] = useState<LevelEvent>({ channels: [] });
  const [startedAtMs, setStartedAtMs] = useState<number | null>(null);
  const [elapsedSec, setElapsedSec] = useState(0);
  // Привязка к делу (этап 05): `resolved` из кэша или `manual` (pending).
  // Запись не блокируется её отсутствием — можно привязать позже.
  const [binding, setBinding] = useState<AdjudicationRef | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Готовность первичной загрузки (чтобы мониторинг не стартовал до резолва).
  const [loaded, setLoaded] = useState(false);
  // Запрошенное подтверждение действия (стоп/пауза) — модальное окно.
  const [confirm, setConfirm] = useState<null | 'stop' | 'pause'>(null);
  // Прогресс активной сессии (каталог + число записанных сегментов) —
  // наглядное подтверждение, что запись действительно идёт.
  const [sessionInfo, setSessionInfo] = useState<SessionInfo | null>(null);

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

  // Первичная загрузка + восстановление состояния идущей записи. Запись живёт в
  // фоне (ядре), а компонент экрана размонтируется при переходе между вкладками
  // — поэтому статус берём из `capture_status`, а не считаем «idle».
  useEffect(() => {
    let active = true;
    Promise.all([listAudioDevices(), getSettings(), scanRecoverable(), getCaptureStatus()])
      .then(([devs, s, rec, status]) => {
        if (!active) return;
        setDevices(devs);
        setSettings(s);
        setRecoverable(rec);
        const preferred =
          s.audio.device ?? devs.find((d) => d.is_default)?.name ?? devs[0]?.name ?? '';
        setDeviceName(preferred);
        if (status.state === 'recording' || status.state === 'paused') {
          setState(status.state);
          setStartedAtMs(status.started_at_unix_ms);
          setSessionInfo({
            output_dir: status.output_dir,
            segment_count: status.segment_count,
          });
        }
        setLoaded(true);
      })
      .catch((e: unknown) => {
        if (active) {
          setError(describeError(e));
          setLoaded(true);
        }
      });
    return () => {
      active = false;
    };
  }, []);

  // Живой мониторинг уровня, пока не идёт запись: показывает, что микрофон
  // работает (открывает устройство → запросит доступ к микрофону). Запись сама
  // эмитит уровни, поэтому монитор нужен только в idle/stopped.
  useEffect(() => {
    if (!loaded) return;
    if (state !== 'idle' && state !== 'stopped') return;
    void startMonitor().catch(() => {});
    return () => {
      void stopMonitor().catch(() => {});
    };
  }, [loaded, state]);

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

  // Хронометр: время от старта сессии (по метке ядра — переживает размонтаж
  // экрана). Тикаем, пока сессия активна (запись/пауза).
  useEffect(() => {
    if ((state !== 'recording' && state !== 'paused') || startedAtMs == null) return;
    const tick = () =>
      setElapsedSec(Math.max(0, Math.floor((Date.now() - startedAtMs) / 1000)));
    tick();
    const id = window.setInterval(tick, CLOCK_TICK_MS);
    return () => window.clearInterval(id);
  }, [state, startedAtMs]);

  // Живой прогресс сессии: опрашиваем число записанных сегментов, пока активны.
  useEffect(() => {
    if (state !== 'recording' && state !== 'paused') return;
    let active = true;
    const poll = () => {
      getCaptureStatus()
        .then((s) => {
          if (active && (s.state === 'recording' || s.state === 'paused')) {
            setSessionInfo({ output_dir: s.output_dir, segment_count: s.segment_count });
          }
        })
        .catch(() => {});
    };
    poll();
    const id = window.setInterval(poll, STATUS_POLL_MS);
    return () => {
      active = false;
      window.clearInterval(id);
    };
  }, [state]);

  // Уточнение/смена привязки уже идущей записи: при изменении привязки во время
  // активной сессии дебаунсим и пишем в манифест (без спама на каждый символ).
  const activeOutputDir = sessionInfo?.output_dir ?? null;
  useEffect(() => {
    if (state !== 'recording' && state !== 'paused') return;
    if (!activeOutputDir) return;
    const id = window.setTimeout(() => {
      bindSessionCase(activeOutputDir, binding).catch((e: unknown) =>
        setError(describeError(e)),
      );
    }, BIND_DEBOUNCE_MS);
    return () => window.clearTimeout(id);
  }, [binding, state, activeOutputDir]);

  const onStart = useCallback(async () => {
    setError(null);
    setElapsedSec(0);
    setDeviceLost(false);
    setDiskWarning(null);
    setMaxDuration(false);
    setStartedAtMs(Date.now());
    try {
      const started = await startCapture();
      setState('recording');
      setSessionInfo({ output_dir: started.output_dir, segment_count: 0 });
      // Привязываем выбранное дело к стартовавшей сессии (ошибка привязки не
      // должна валить запись — её можно повторить/уточнить).
      if (binding) {
        bindSessionCase(started.output_dir, binding).catch((e: unknown) =>
          setError(describeError(e)),
        );
      }
    } catch (e) {
      setStartedAtMs(null);
      setError(describeError(e));
    }
  }, [binding]);

  const onStop = useCallback(async () => {
    // Показываем фазу сохранения сразу, не дожидаясь backend-события
    // `capture_state: stopping`: синхронная команда может задержать доставку
    // события в webview до своего возврата, и индикатор бы не появился.
    setState('stopping');
    // Дать webview отрисовать кадр «Сохранение записи…» ДО вызова stopCapture:
    // финализация (сброс/хеши/журнал) может заблокировать поток, и без
    // гарантированной отрисовки индикатор не успел бы проявиться.
    await nextPaint();
    const startedAt = Date.now();
    try {
      await stopCapture();
      // Короткие записи финализируются мгновенно — держим индикатор минимум
      // заметное время, иначе оператор не успевает увидеть подтверждение.
      await holdAtLeast(startedAt, STOP_INDICATOR_MIN_MS);
      setState('stopped');
      setStartedAtMs(null);
      setSessionInfo(null);
      setLevel({ channels: [] });
    } catch (e) {
      // Стоп забрал сессию из состояния ядра — активной записи уже нет.
      await holdAtLeast(startedAt, STOP_INDICATOR_MIN_MS);
      setState('stopped');
      setStartedAtMs(null);
      setSessionInfo(null);
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

  // Горячие клавиши: R — старт, S — стоп (с подтверждением), Пробел — пауза
  // (с подтверждением) / возобновление. Не перехватываем при фокусе в поле
  // ввода и при открытом окне подтверждения.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) return;
      if (confirm) return;
      const k = e.key.toLowerCase();
      if (k === 'r' && (state === 'idle' || state === 'stopped')) {
        e.preventDefault();
        void onStart();
      } else if (k === 's' && (state === 'recording' || state === 'paused')) {
        e.preventDefault();
        setConfirm('stop');
      } else if (e.key === ' ' && state === 'recording') {
        e.preventDefault();
        setConfirm('pause');
      } else if (e.key === ' ' && state === 'paused') {
        e.preventDefault();
        void onResume();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [state, confirm, onStart, onResume]);

  const isActive = state === 'recording' || state === 'paused';
  const quality = settings
    ? `${formatHz(settings.audio.sample_rate_hz)} · ${settings.audio.bit_depth} бит · ${formatChannels(settings.audio.channels)}`
    : '—';
  // Текущая (идущая) сессия не предлагается к восстановлению — она уже активна.
  const activeDir = sessionInfo?.output_dir ?? null;
  const pendingRecoverable = recoverable.filter((r) => r.dir !== activeDir);

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

      {/* Баннер восстановления — решение из этапа 02 (продолжить/закрыть).
          Текущую идущую сессию сюда не показываем (она уже активна). */}
      {pendingRecoverable.length > 0 && (
        <Card variant="accent">
          <BlockHead
            numeral="!"
            title="Найдена незавершённая сессия"
            hint="После сбоя осталась запись. Продолжить её восстановление или закрыть."
          />
          {pendingRecoverable.map((r) => (
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
                style={NEUTRAL_BTN}
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
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              marginBottom: 8,
            }}
          >
            <span style={{ ...fieldLabelStyle, marginBottom: 0 }}>Уровень сигнала</span>
            {/* Дубль статуса записи рядом с индикатором уровня. */}
            <Tag tone={STATUS_TONE[state]} role="status" aria-live="polite">
              {isRecordingDot(state)}
              {state === 'idle' || state === 'stopped' ? 'Мониторинг' : STATUS_LABEL[state]}
            </Tag>
          </div>
          <LevelMeter level={level} active={state === 'recording'} />
        </div>

        {sessionInfo && (state === 'recording' || state === 'paused') && (
          <p
            aria-live="polite"
            style={{ fontSize: 12, color: 'var(--ink-soft)', marginTop: 16, marginBottom: 0 }}
          >
            Пишется в:{' '}
            <span className="num" style={{ wordBreak: 'break-all' }}>
              {sessionInfo.output_dir ?? '—'}
            </span>{' '}
            · сегментов записано:{' '}
            <span className="num">{sessionInfo.segment_count}</span>
          </p>
        )}
      </Card>

      <Card>
        <BlockHead
          numeral="B"
          title="Привязка к делу"
          hint="Выберите дело из кэша докета (оффлайн) или введите № вручную"
        />
        <CasePicker binding={binding} onChange={setBinding} />
        {isActive && !binding && (
          <p
            aria-live="polite"
            style={{ fontSize: 12, color: 'var(--accent-deep)', margin: '12px 0 0' }}
          >
            ⚠ Запись идёт без привязки к делу — привяжите дело, чтобы запись
            связалась с производством на сервере.
          </p>
        )}
      </Card>

      {/* Фаза сохранения после стопа: финализация может занять время —
          показываем прогресс, чтобы станция не казалась «зависшей». */}
      {state === 'stopping' && (
        <Card variant="accent" role="status" aria-live="polite">
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 10 }}>
            <Tag tone="accent">Сохранение записи…</Tag>
          </div>
          <ProgressBar label="Сохранение записи" />
          <p style={{ fontSize: 12, color: 'var(--ink-soft)', margin: '10px 0 0' }}>
            Финализация сегментов и контроль целостности (хеши, журнал). Не
            выключайте станцию.
          </p>
        </Card>
      )}

      <div style={{ display: 'flex', gap: 12, flexWrap: 'wrap' }}>
        {(state === 'idle' || state === 'stopped') && (
          <Button variant="primary" onClick={() => void onStart()} disabled={!deviceName}>
            ● Старт записи
          </Button>
        )}
        {state === 'recording' && (
          <Button variant="secondary" style={NEUTRAL_BTN} onClick={() => setConfirm('pause')}>
            ❚❚ Пауза
          </Button>
        )}
        {state === 'paused' && (
          <Button variant="secondary" style={NEUTRAL_BTN} onClick={() => void onResume()}>
            ▶ Возобновить
          </Button>
        )}
        {isActive && (
          <Button variant="primary" onClick={() => setConfirm('stop')}>
            ■ Стоп
          </Button>
        )}
      </div>

      <p style={{ fontSize: 12, color: 'var(--muted)', margin: 0 }}>
        Горячие клавиши: <kbd>R</kbd> — старт, <kbd>S</kbd> — стоп,{' '}
        <kbd>Пробел</kbd> — пауза/возобновление.
      </p>

      <ConfirmDialog
        open={confirm === 'stop'}
        title="Остановить запись?"
        description="Запись сессии будет завершена и сегменты финализированы. Возобновить эту же сессию после остановки нельзя."
        confirmLabel="Остановить"
        tone="danger"
        onConfirm={() => {
          setConfirm(null);
          void onStop();
        }}
        onCancel={() => setConfirm(null)}
      />
      <ConfirmDialog
        open={confirm === 'pause'}
        title="Поставить запись на паузу?"
        description="Захват приостановится. Сегменты уже на диске сохранены; запись можно возобновить."
        confirmLabel="Поставить на паузу"
        tone="neutral"
        onConfirm={() => {
          setConfirm(null);
          void onPause();
        }}
        onCancel={() => setConfirm(null)}
      />
    </div>
  );
}

// Преобразование линейной амплитуды [0..1] в проценты шкалы по дБFS: тихая речь
// в линейной шкале почти не видна, логарифм делает метр «живым».
function toMeterPct(v: number): number {
  if (v <= 0) return 0;
  const db = 20 * Math.log10(v); // dBFS, ≤ 0
  const pct = ((db - METER_FLOOR_DBFS) / (0 - METER_FLOOR_DBFS)) * 100;
  return Math.max(0, Math.min(100, pct));
}

// ── Индикатор уровня: столбик RMS/пик + клиппинг на каждый канал записи ──────
// Каналов может быть несколько (многоканал) — рисуем по столбику на канал.
function LevelMeter({ level, active }: { level: LevelEvent; active: boolean }) {
  const channels = level.channels.length > 0 ? level.channels : [{ peak: 0, rms: 0 }];
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      {channels.map((ch, i) => (
        <ChannelMeter key={i} index={i} total={channels.length} ch={ch} active={active} />
      ))}
    </div>
  );
}

function ChannelMeter({
  index,
  total,
  ch,
  active,
}: {
  index: number;
  total: number;
  ch: ChannelLevel;
  active: boolean;
}) {
  const rmsPct = toMeterPct(ch.rms);
  const peakPct = toMeterPct(ch.peak);
  const clipping = active && ch.peak >= FULL_SCALE * CLIP_RATIO;
  return (
    <div
      role="meter"
      aria-label={total > 1 ? `Уровень канала ${index + 1}` : 'Индикатор уровня сигнала'}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(rmsPct)}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        {total > 1 && (
          <span className="num" style={{ fontSize: 11, color: 'var(--muted)', width: 28 }}>
            К{index + 1}
          </span>
        )}
        <div
          style={{
            position: 'relative',
            flex: 1,
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
        {clipping && <Tag tone="accent">Клиппинг</Tag>}
      </div>
      <span className="num" style={{ fontSize: 11, color: 'var(--muted)' }}>
        RMS {Math.round(rmsPct)}% · пик {Math.round(peakPct)}%
      </span>
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

// Дождаться отрисовки кадра (двойной requestAnimationFrame — после commit'а
// React браузер успевает покрасить). Фолбэк по таймеру — для сред без rAF
// (тесты) и чтобы не зависнуть дольше одного кадра.
function nextPaint(): Promise<void> {
  return new Promise((resolve) => {
    let settled = false;
    const done = () => {
      if (!settled) {
        settled = true;
        resolve();
      }
    };
    if (typeof requestAnimationFrame === 'function') {
      requestAnimationFrame(() => requestAnimationFrame(done));
    }
    setTimeout(done, 32);
  });
}

// Подождать, пока с момента `startedAt` пройдёт хотя бы `minMs` (минимальная
// видимость индикатора). Если время уже вышло — не ждём.
function holdAtLeast(startedAt: number, minMs: number): Promise<void> {
  const remaining = minMs - (Date.now() - startedAt);
  if (remaining <= 0) return Promise.resolve();
  return new Promise((resolve) => setTimeout(resolve, remaining));
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}
