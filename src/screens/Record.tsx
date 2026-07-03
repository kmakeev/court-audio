import { useCallback, useEffect, useMemo, useState } from 'react';
import type { CSSProperties, Dispatch, SetStateAction } from 'react';
import {
  BlockHead,
  Button,
  Card,
  CriticalNotice,
  fieldCaptionStyle,
  NEUTRAL_BTN,
  ProgressBar,
  screenStackStyle,
  Select,
  Tag,
} from '../design';
import { CLIP_RATIO, formatClock, FULL_SCALE, toMeterPct } from '../lib/format';
import {
  RECORDING_STATUS_LABEL,
  RECORDING_STATUS_TONE,
} from '../lib/recording-status';
import { CasePicker } from '../components/CasePicker';
import { SelfTestPanel } from '../components/SelfTest';
import { ConfirmDialog } from '../shell/ConfirmDialog';
import {
  addMarker,
  bindSessionCase,
  discardSession,
  editMarker,
  endRoleSpan,
  getCaptureStatus,
  listAnnotations,
  listAudioDevices,
  onAudioLevel,
  onCaptureState,
  onReliabilityWarning,
  pauseCapture,
  recoverSession,
  removeMarker,
  resumeCapture,
  scanRecoverable,
  startCapture,
  startMonitor,
  startRoleSpan,
  stopCapture,
  stopMonitor,
  type AdjudicationRef,
  type AnnotationSnapshot,
  type CaptureStateValue,
  type ChannelLevel,
  type DeviceInfo,
  type LevelEvent,
  type RecoverableSession,
  type ReliabilityEvent,
} from '../lib/core';
import { getSettings, type Settings } from '../lib/settings';
import { maybeBeep, reliabilityToAlert } from '../lib/alerts';
import { humanizeError } from '../lib/errors';
import { useAuth } from '../lib/auth-context';

// Экран «Запись» (этап 04). UI станции захвата: устройство, живые индикаторы
// уровня, управление сессией и недвусмысленный статус. Вся логика — в ядре
// (этапы 01–03); здесь только команды и отображение событий.

// ── Константы отображения (НЕ бизнес-параметры реестра) ──────────────────────
// Косметика метра/хронометра (шкала дБFS, клиппинг, формат времени) вынесена в
// общий `lib/format` — её переиспользуют шапка, «режим зала» и компакт-оверлей.
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

// Кнопки управления записью в одном ряду: primary (Старт/Стоп) в DS высотой 44,
// secondary (Пауза/Возобновить) — 38. Приводим нейтральные к той же высоте, чтобы
// ряд не «прыгал» и кнопки были одного размера.
const CONTROL_BTN: CSSProperties = { ...NEUTRAL_BTN, height: 44 };

interface SessionInfo {
  output_dir: string | null;
  segment_count: number;
}

// Метки/тона статуса записи — общий источник (шапка/режим зала/оверлей).
type UiState = CaptureStateValue;
const STATUS_LABEL = RECORDING_STATUS_LABEL;
const STATUS_TONE = RECORDING_STATUS_TONE;

export function RecordScreen() {
  // Гейт входа (этап 10.3): без вошедшего оператора старт новой сессии закрыт
  // (backend тоже отклонит). `status === null` (вне провайдера/до загрузки) —
  // не блокируем, чтобы не мешать изолированным сценариям.
  const { status } = useAuth();
  const operatorMissing = status !== null && !status.operator;
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [deviceName, setDeviceName] = useState('');
  const [settings, setSettings] = useState<Settings | null>(null);
  const [state, setState] = useState<UiState>('idle');
  // Фаза сохранения записи (финализация после стопа). Отдельный флаг, а не
  // `state==='stopping'`: backend-событие `capture_state: stopped` приходит сразу
  // по завершении stopCapture и иначе мгновенно сбросило бы индикатор. Флагом
  // управляет только onStop — держим его минимум заметное время.
  const [saving, setSaving] = useState(false);
  // Уровни по дорожкам: track_id → последнее событие (многоканал — этап 09).
  // Для одноканальной записи здесь одна запись с ключом 0.
  const [levels, setLevels] = useState<Record<number, LevelEvent>>({});
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
  // Панель «Проверка перед заседанием» (self-test, этап 10.6): по кнопке, до старта.
  const [showSelfTest, setShowSelfTest] = useState(false);

  // Активные предупреждения надёжности (по событиям ядра).
  const [deviceLost, setDeviceLost] = useState(false);
  const [diskWarning, setDiskWarning] = useState<ReliabilityEvent | null>(null);
  const [maxDuration, setMaxDuration] = useState(false);

  // Незавершённые сессии для восстановления (скан при монтировании).
  const [recoverable, setRecoverable] = useState<RecoverableSession[]>([]);

  const soundAlerts = settings?.ux.sound_alerts.enabled ?? false;
  const handleWarning = useCallback(
    (e: ReliabilityEvent) => {
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
      // Опциональный звуковой сигнал о сбое (этап 10.6): дублирует баннер выше.
      const alert = reliabilityToAlert(e);
      if (alert) maybeBeep(soundAlerts, alert);
    },
    [soundAlerts],
  );

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
      const a = await onAudioLevel((e) =>
        setLevels((prev) => ({ ...prev, [e.track_id ?? 0]: e })),
      );
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
    setSaving(true);
    const startedAt = Date.now();
    try {
      await stopCapture();
      // Короткие записи финализируются мгновенно — держим индикатор минимум
      // заметное время, иначе оператор не успевает увидеть подтверждение.
      await holdAtLeast(startedAt, STOP_INDICATOR_MIN_MS);
      setState('stopped');
      setStartedAtMs(null);
      setSessionInfo(null);
      setLevels({});
    } catch (e) {
      // Стоп забрал сессию из состояния ядра — активной записи уже нет.
      await holdAtLeast(startedAt, STOP_INDICATOR_MIN_MS);
      setState('stopped');
      setStartedAtMs(null);
      setSessionInfo(null);
      setError(describeError(e));
    } finally {
      setSaving(false);
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
    <div style={screenStackStyle(880)}>
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
          {!isActive && !saving && (
            <Button
              variant="secondary"
              style={{ ...NEUTRAL_BTN, marginLeft: 'auto' }}
              onClick={() => setShowSelfTest((v) => !v)}
              aria-expanded={showSelfTest}
            >
              {showSelfTest ? 'Скрыть проверку' : 'Проверка перед заседанием'}
            </Button>
          )}
        </div>
      </Card>

      {/* Проверка перед заседанием (self-test, этап 10.6): по кнопке, до старта. */}
      {showSelfTest && !isActive && <SelfTestPanel numeral="✓" />}

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
          <TrackMeters
            levels={levels}
            tracks={settings?.audio.multichannel.enabled ? settings.audio.tracks : []}
            active={state === 'recording'}
          />
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

      {/* Живая разметка (этап 10): закладки и роли по ходу заседания. Видна
          только при активной сессии; действия идут вне аудио-потока. */}
      {isActive && settings && (
        <LiveAnnotations
          settings={settings}
          levels={levels}
          onError={setError}
        />
      )}

      {/* Фаза сохранения после стопа: финализация может занять время —
          показываем прогресс, чтобы станция не казалась «зависшей». */}
      {saving && (
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
        {(state === 'idle' || state === 'stopped') && !saving && (
          <Button
            variant="primary"
            onClick={() => void onStart()}
            disabled={!deviceName || operatorMissing}
          >
            ● Старт записи
          </Button>
        )}
        {state === 'recording' && (
          <Button variant="secondary" style={CONTROL_BTN} onClick={() => setConfirm('pause')}>
            ❚❚ Пауза
          </Button>
        )}
        {state === 'paused' && (
          <Button variant="secondary" style={CONTROL_BTN} onClick={() => void onResume()}>
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

// ── Живая разметка: закладки + интервалы ролей (этап 10) ─────────────────────
// Кнопки категорий/ролей с минимальным трением (горячие клавиши цифрами),
// список меток с правкой/удалением, открытые интервалы ролей. Все действия —
// асинхронные команды ядра (вне аудио-потока), результат — свёрнутый снимок.
function LiveAnnotations({
  settings,
  levels,
  onError,
}: {
  settings: Settings;
  levels: Record<number, LevelEvent>;
  onError: (e: string | null) => void;
}) {
  const [ann, setAnn] = useState<AnnotationSnapshot>({ markers: [], role_spans: [] });
  const [comment, setComment] = useState('');
  const categories = settings.markers.categories;
  const roles = settings.audio.roles;
  const multichannel = settings.audio.multichannel.enabled && settings.audio.tracks.length > 0;

  // Подтянуть текущую разметку при появлении карты (сессия могла идти в фоне).
  useEffect(() => {
    listAnnotations()
      .then(setAnn)
      .catch(() => {});
  }, []);

  // Выполнить команду разметки: снимок заменяет состояние, ошибка — в баннер.
  const run = useCallback(
    (p: Promise<AnnotationSnapshot>) => {
      p.then(setAnn).catch((e: unknown) => onError(describeError(e)));
    },
    [onError],
  );

  const onAddMarker = useCallback(
    (category: string) => {
      run(addMarker(category, comment || null));
      setComment('');
    },
    [run, comment],
  );

  // Отмена последней метки (этап 10.6, мелочь трения): удаляем самую позднюю метку
  // (наибольшее смещение — при живой записи это последняя поставленная). Удаление
  // журналируется как действие под хеш-цепочкой (как правка — целостность разметки).
  const lastMarkerId =
    ann.markers.length > 0 ? ann.markers[ann.markers.length - 1].id : null;
  const undoLastMarker = useCallback(() => {
    if (lastMarkerId) run(removeMarker(lastMarkerId));
  }, [lastMarkerId, run]);

  // Активная (самая громкая) дорожка — для подстановки роли (многоканал).
  const activeTrackId = useMemo(() => {
    let best: number | null = null;
    let bestRms = -1;
    for (const [id, ev] of Object.entries(levels)) {
      const rms = ev.channels.reduce((m, c) => Math.max(m, c.rms), 0);
      if (rms > bestRms) {
        bestRms = rms;
        best = Number(id);
      }
    }
    return best;
  }, [levels]);
  const suggestedRole =
    multichannel && activeTrackId != null ? settings.audio.tracks[activeTrackId]?.role : undefined;

  // Горячие клавиши: цифры 1..N — категории закладок; Backspace — отмена последней
  // метки. Не перехватываем при фокусе в поле ввода (комментарий/№ дела).
  // Совместимо с R/S/Пробел выше.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) return;
      if (e.key === 'Backspace') {
        e.preventDefault();
        undoLastMarker();
        return;
      }
      const n = Number(e.key);
      if (Number.isInteger(n) && n >= 1 && n <= categories.length) {
        e.preventDefault();
        onAddMarker(categories[n - 1]);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [categories, onAddMarker, undoLastMarker]);

  const openSpans = ann.role_spans.filter((s) => s.end_offset_ms == null);

  return (
    <Card>
      <BlockHead
        numeral="C"
        title="Живая разметка"
        hint="Закладки и роли говорящих по ходу заседания — подсказки для протокола"
      />

      {/* Комментарий к следующей закладке (опционально). */}
      <label style={{ display: 'block', marginTop: 12 }}>
        <span style={fieldLabelStyle}>Комментарий к закладке (необязательно)</span>
        <input
          aria-label="Комментарий к закладке"
          value={comment}
          onChange={(e) => setComment(e.target.value)}
          placeholder="например: реплика свидетеля"
          style={{
            width: '100%',
            padding: '8px 10px',
            border: '1px solid var(--hairline)',
            background: 'var(--paper)',
            color: 'var(--ink)',
            fontSize: 13,
          }}
        />
      </label>

      {/* Кнопки категорий (горячие клавиши — цифры). */}
      <div style={{ marginTop: 14 }}>
        <span style={fieldLabelStyle}>Поставить закладку</span>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          {categories.map((c, i) => (
            <Button
              key={c}
              variant="secondary"
              style={NEUTRAL_BTN}
              onClick={() => onAddMarker(c)}
            >
              {i < 9 ? `${i + 1} · ` : ''}
              {c}
            </Button>
          ))}
          {categories.length === 0 && (
            <span style={{ fontSize: 12, color: 'var(--muted)' }}>
              Категории не заданы — настройте справочник в «Настройках».
            </span>
          )}
          {lastMarkerId && (
            <Button
              variant="secondary"
              style={NEUTRAL_BTN}
              onClick={undoLastMarker}
              title="Отменить последнюю метку (Backspace)"
            >
              ⌫ Отменить последнюю метку
            </Button>
          )}
        </div>
      </div>

      {/* Кнопки ролей + подстановка активной дорожки (многоканал). */}
      <div style={{ marginTop: 16 }}>
        <span style={fieldLabelStyle}>Отметить, кто говорит</span>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          {suggestedRole && activeTrackId != null && (
            <Button variant="primary" onClick={() => run(startRoleSpan(null, activeTrackId))}>
              ● Активная дорожка: {suggestedRole}
            </Button>
          )}
          {roles.map((r) => (
            <Button key={r} variant="secondary" style={NEUTRAL_BTN} onClick={() => run(startRoleSpan(r))}>
              {r}
            </Button>
          ))}
        </div>
      </div>

      {/* Открытые интервалы ролей — можно завершить. */}
      {openSpans.length > 0 && (
        <div style={{ marginTop: 16 }}>
          <span style={fieldLabelStyle}>Идёт речь</span>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {openSpans.map((s) => (
              <div key={s.id} style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                <Tag tone="accent">{s.role}</Tag>
                <span className="num" style={{ fontSize: 12, color: 'var(--ink-soft)', flex: 1 }}>
                  с {formatClock(Math.floor(s.start_offset_ms / 1000))}
                </span>
                <Button variant="secondary" style={NEUTRAL_BTN} onClick={() => run(endRoleSpan(s.id))}>
                  Завершить
                </Button>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Список меток с правкой категории и удалением (таймлайн-обзор). */}
      <div style={{ marginTop: 16 }}>
        <span style={fieldLabelStyle}>Метки сессии ({ann.markers.length})</span>
        {ann.markers.length === 0 ? (
          <p style={{ fontSize: 12, color: 'var(--muted)', margin: 0 }}>Меток пока нет.</p>
        ) : (
          <ul style={{ listStyle: 'none', margin: 0, padding: 0, display: 'flex', flexDirection: 'column', gap: 8 }}>
            {ann.markers.map((m) => (
              <li
                key={m.id}
                style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}
              >
                <span className="num" style={{ fontSize: 12, color: 'var(--ink)', width: 72 }}>
                  {formatClock(Math.floor(m.offset_ms / 1000))}
                </span>
                <div style={{ minWidth: 180 }}>
                  <Select
                    ariaLabel={`Категория метки ${formatClock(Math.floor(m.offset_ms / 1000))}`}
                    value={m.category}
                    onChange={(v) => run(editMarker(m.id, v, m.comment ?? null))}
                    options={categoryOptions(categories, m.category)}
                  />
                </div>
                {m.comment && (
                  <span style={{ fontSize: 12, color: 'var(--ink-soft)', flex: 1 }}>{m.comment}</span>
                )}
                <Button variant="secondary" style={NEUTRAL_BTN} onClick={() => run(removeMarker(m.id))}>
                  Удалить
                </Button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </Card>
  );
}

// Опции селекта категории метки: справочник + текущая категория, если её уже нет
// в справочнике (не теряем историческое значение при правке).
function categoryOptions(categories: string[], current: string) {
  const opts = categories.map((c) => ({ value: c, label: c }));
  if (current && !categories.includes(current)) {
    opts.unshift({ value: current, label: `${current} (вне справочника)` });
  }
  return opts;
}

// ── Пофайловые метры по дорожкам (многоканал — этап 09) ──────────────────────
// Одноканальная запись (track_id 0) рисуется одним метром без подписи роли.
function TrackMeters({
  levels,
  tracks,
  active,
}: {
  levels: Record<number, LevelEvent>;
  tracks: { role: string; label: string }[];
  active: boolean;
}) {
  // Дорожки для показа: если задана карта дорожек — по ней (даже до первых
  // событий), иначе — по пришедшим событиям (v1: одна дорожка 0).
  const ids =
    tracks.length > 0
      ? tracks.map((_, i) => i)
      : Object.keys(levels)
          .map(Number)
          .sort((a, b) => a - b);
  const list = ids.length > 0 ? ids : [0];

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      {list.map((id) => {
        const t = tracks[id];
        const caption = t ? t.label.trim() || t.role : null;
        return (
          <div key={id} style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
            {caption && (
              <span
                style={{
                  fontSize: 11,
                  textTransform: 'uppercase',
                  letterSpacing: '0.12em',
                  color: 'var(--muted)',
                  fontWeight: 500,
                }}
              >
                {`Дорожка ${id + 1} · ${caption}`}
              </span>
            )}
            <LevelMeter level={levels[id] ?? { channels: [] }} active={active} />
          </div>
        );
      })}
    </div>
  );
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
          {/* Заливка RMS (плавность — класс с поддержкой prefers-reduced-motion) */}
          <div
            className="level-meter-fill"
            style={{
              position: 'absolute',
              inset: 0,
              width: `${rmsPct}%`,
              background: clipping ? 'var(--accent)' : 'var(--green)',
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

const fieldLabelStyle: CSSProperties = {
  display: 'block',
  marginBottom: 8,
  ...fieldCaptionStyle,
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

function formatHz(hz: number): string {
  return `${(hz / 1000).toFixed(1).replace(/\.0$/, '')} кГц`;
}

function formatChannels(n: number): string {
  if (n === 1) return 'моно';
  if (n === 2) return 'стерео';
  return `${n} кан.`;
}

// Подождать, пока с момента `startedAt` пройдёт хотя бы `minMs` (минимальная
// видимость индикатора). Если время уже вышло — не ждём.
function holdAtLeast(startedAt: number, minMs: number): Promise<void> {
  const remaining = minMs - (Date.now() - startedAt);
  if (remaining <= 0) return Promise.resolve();
  return new Promise((resolve) => setTimeout(resolve, remaining));
}

// Человекочитаемый текст ошибки ядра (этап 10.6, словарь `lib/errors`).
function describeError(e: unknown): string {
  return humanizeError(e);
}
