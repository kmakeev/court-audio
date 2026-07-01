import { useEffect, useState, type CSSProperties, type ReactNode } from 'react';
import { BlockHead, Button, Card, Checkbox, Field, Icon, InfoTip, Select, Tag } from '../design';
import { listAudioDevices, type DeviceInfo } from '../lib/core';
import {
  getSettings,
  saveSettings,
  type RetentionMode,
  type Settings,
  type TrackConfig,
} from '../lib/settings';

// Экран «Настройки» (этап 04). Полное покрытие реестра docs/configuration.md.
// Две визуально разделённые секции — «Оператор» (рабочие параметры) и
// «Администратор» (инфраструктура/безопасность) — решение заказчика. Дефолты
// задаёт ядро (Rust по реестру); UI их только читает/правит/сохраняет.

type Status =
  | { kind: 'loading' }
  | { kind: 'ready' }
  | { kind: 'saving' }
  | { kind: 'saved' }
  | { kind: 'error'; message: string };

const RETENTION_LABELS: Record<RetentionMode, string> = {
  until_confirmed_plus_window: 'До подтверждения + буферное окно',
  delete_on_confirm: 'Удалять сразу по подтверждению',
  manual: 'До ручного удаления',
};

export function SettingsScreen() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [status, setStatus] = useState<Status>({ kind: 'loading' });
  // Список устройств ввода — для выбора источника дорожек (многоканал).
  const [devices, setDevices] = useState<DeviceInfo[]>([]);

  useEffect(() => {
    getSettings()
      .then((s) => {
        setSettings(s);
        setStatus({ kind: 'ready' });
      })
      .catch((e: unknown) => setStatus({ kind: 'error', message: describeError(e) }));
    // Устройства подгружаем best-effort: их отсутствие не ломает форму.
    listAudioDevices()
      .then(setDevices)
      .catch(() => setDevices([]));
  }, []);

  function update(mut: (draft: Settings) => void) {
    setSettings((prev) => {
      if (!prev) return prev;
      const next = structuredClone(prev);
      mut(next);
      return next;
    });
    setStatus({ kind: 'ready' });
  }

  const errors = settings ? validate(settings) : {};
  const hasErrors = Object.keys(errors).length > 0;

  async function onSave() {
    if (!settings || hasErrors) return;
    setStatus({ kind: 'saving' });
    try {
      await saveSettings(settings);
      setStatus({ kind: 'saved' });
    } catch (e) {
      setStatus({ kind: 'error', message: describeError(e) });
    }
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 760 }}>
      <Card>
        <BlockHead
          numeral="03"
          title="Настройки станции"
          hint="Значения по умолчанию — из реестра docs/configuration.md"
        />
        {status.kind === 'error' && (
          <div style={{ marginTop: 8 }}>
            <Tag tone="accent">Ошибка: {status.message}</Tag>
          </div>
        )}
      </Card>

      {settings && (
        <>
          {/* ─────────────── СЕКЦИЯ ОПЕРАТОРА ─────────────── */}
          <SectionTitle>Оператор · рабочие параметры</SectionTitle>

          <Card>
            <BlockHead numeral="A" title="Аудио" />
            <Grid>
              <Field
                label="Устройство ввода"
                placeholder="Системное по умолчанию"
                value={settings.audio.device ?? ''}
                onChange={(e) =>
                  update((d) => {
                    d.audio.device = e.target.value.trim() === '' ? null : e.target.value;
                  })
                }
              />
              <NumField
                label="Частота, Гц"
                value={settings.audio.sample_rate_hz}
                error={errors['sample_rate_hz']}
                onChange={(v) => update((d) => { d.audio.sample_rate_hz = v; })}
              />
              <NumField
                label="Разрядность, бит"
                value={settings.audio.bit_depth}
                error={errors['bit_depth']}
                onChange={(v) => update((d) => { d.audio.bit_depth = v; })}
              />
              <NumField
                label="Каналы"
                value={settings.audio.channels}
                error={errors['channels']}
                onChange={(v) => update((d) => { d.audio.channels = v; })}
              />
            </Grid>
            <Grid>
              <Labeled label="Архивная копия (FLAC)">
                <Checkbox
                  checked={settings.audio.archive_copy.enabled}
                  onChange={(e) =>
                    update((d) => { d.audio.archive_copy.enabled = e.target.checked; })
                  }
                >
                  Включить сжатую копию для архива
                </Checkbox>
              </Labeled>
            </Grid>
          </Card>

          <MultichannelCard
            settings={settings}
            errors={errors}
            devices={devices}
            update={update}
          />

          {/* ─────────────── СЕКЦИЯ АДМИНИСТРАТОРА ─────────────── */}
          <SectionTitle>
            Администратор · инфраструктура и безопасность
          </SectionTitle>

          <Card>
            <BlockHead numeral="B" title="Запись и надёжность" />
            <Grid>
              <NumField
                label="Длина сегмента, с"
                value={settings.recorder.segment_seconds}
                error={errors['segment_seconds']}
                onChange={(v) => update((d) => { d.recorder.segment_seconds = v; })}
              />
              <NumField
                label="Интервал fsync, мс"
                value={settings.recorder.flush_interval_ms}
                error={errors['flush_interval_ms']}
                onChange={(v) => update((d) => { d.recorder.flush_interval_ms = v; })}
              />
              <NumField
                label="Макс. длительность, ч"
                value={settings.recorder.max_session_hours}
                onChange={(v) => update((d) => { d.recorder.max_session_hours = v; })}
              />
              <NumField
                label="Watchdog таймаут, мс"
                value={settings.reliability.watchdog_timeout_ms}
                onChange={(v) => update((d) => { d.reliability.watchdog_timeout_ms = v; })}
              />
              <NumField
                label="Порог «мало места», МБ"
                value={settings.reliability.disk_low_threshold_mb}
                error={errors['disk_low']}
                onChange={(v) => update((d) => { d.reliability.disk_low_threshold_mb = v; })}
              />
              <NumField
                label="Критич. порог места, МБ"
                value={settings.reliability.disk_critical_mb}
                error={errors['disk_critical']}
                onChange={(v) => update((d) => { d.reliability.disk_critical_mb = v; })}
              />
            </Grid>
            <Grid>
              <Labeled label="Авто-возобновление">
                <Checkbox
                  checked={settings.reliability.device_reconnect.auto_resume}
                  onChange={(e) =>
                    update((d) => {
                      d.reliability.device_reconnect.auto_resume = e.target.checked;
                    })
                  }
                >
                  При возврате устройства
                </Checkbox>
              </Labeled>
              <Labeled label="Дублирующая дорожка">
                <Checkbox
                  checked={settings.reliability.mirror.enabled}
                  onChange={(e) =>
                    update((d) => { d.reliability.mirror.enabled = e.target.checked; })
                  }
                >
                  Зеркало на второй носитель
                </Checkbox>
              </Labeled>
              {settings.reliability.mirror.enabled && (
                <Field
                  label="Путь зеркала"
                  placeholder="/mnt/backup/recordings"
                  value={settings.reliability.mirror.path ?? ''}
                  error={errors['mirror_path']}
                  onChange={(e) =>
                    update((d) => {
                      d.reliability.mirror.path =
                        e.target.value.trim() === '' ? null : e.target.value;
                    })
                  }
                />
              )}
            </Grid>
          </Card>

          <Card>
            <BlockHead numeral="C" title="Хранилище и ретеншн" />
            <Grid>
              <Field
                label="Корень хранилища"
                placeholder="<data-dir>/recordings"
                value={settings.storage.root_path ?? ''}
                onChange={(e) =>
                  update((d) => {
                    d.storage.root_path = e.target.value.trim() === '' ? null : e.target.value;
                  })
                }
              />
              <Labeled label="Политика удаления">
                <Select
                  ariaLabel="Политика удаления"
                  value={settings.retention.mode}
                  onChange={(v) => update((d) => { d.retention.mode = v as RetentionMode; })}
                  options={(Object.keys(RETENTION_LABELS) as RetentionMode[]).map((m) => ({
                    value: m,
                    label: RETENTION_LABELS[m],
                  }))}
                />
              </Labeled>
              <NumField
                label="Буферное окно, ч"
                value={settings.retention.safety_window_hours}
                onChange={(v) => update((d) => { d.retention.safety_window_hours = v; })}
              />
            </Grid>
            <Grid>
              <Labeled label="Шифрование at-rest">
                <Checkbox
                  checked={settings.storage.encrypt_at_rest}
                  onChange={(e) =>
                    update((d) => { d.storage.encrypt_at_rest = e.target.checked; })
                  }
                >
                  AES-256-GCM на сегмент
                </Checkbox>
              </Labeled>
              <Labeled label="Удалять только после серверной проверки">
                <Checkbox
                  checked={settings.retention.require_integrity_verified}
                  onChange={(e) =>
                    update((d) => {
                      d.retention.require_integrity_verified = e.target.checked;
                    })
                  }
                >
                  integrity_verified=true
                </Checkbox>
              </Labeled>
            </Grid>
          </Card>

          <Card>
            <BlockHead numeral="D" title="Сеть и выгрузка" />
            <Grid>
              <Field
                label="URL сервера ex_system"
                placeholder="https://…"
                value={settings.sync.server_base_url ?? ''}
                error={errors['server_base_url']}
                onChange={(e) =>
                  update((d) => {
                    d.sync.server_base_url =
                      e.target.value.trim() === '' ? null : e.target.value;
                  })
                }
              />
              <NumField
                label="Размер чанка, МБ"
                value={settings.sync.chunk_size_mb}
                error={errors['chunk_size_mb']}
                onChange={(v) => update((d) => { d.sync.chunk_size_mb = v; })}
              />
              <NumField
                label="Параллельных выгрузок"
                value={settings.sync.parallel_uploads}
                onChange={(v) => update((d) => { d.sync.parallel_uploads = v; })}
              />
              <NumField
                label="Бэкофф база, мс"
                value={settings.sync.retry.backoff_base_ms}
                onChange={(v) => update((d) => { d.sync.retry.backoff_base_ms = v; })}
              />
              <NumField
                label="Бэкофф потолок, мс"
                value={settings.sync.retry.backoff_max_ms}
                onChange={(v) => update((d) => { d.sync.retry.backoff_max_ms = v; })}
              />
              <NumField
                label="Лимит попыток (0 = ∞)"
                value={settings.sync.retry.max_attempts}
                onChange={(v) => update((d) => { d.sync.retry.max_attempts = v; })}
              />
            </Grid>
            <Grid>
              <Labeled label="Авто-выгрузка">
                <Checkbox
                  checked={settings.sync.auto_upload}
                  onChange={(e) => update((d) => { d.sync.auto_upload = e.target.checked; })}
                >
                  По готовности записи (фон)
                </Checkbox>
              </Labeled>
              <Labeled label="Откладывать при записи">
                <Checkbox
                  checked={settings.sync.defer_during_recording}
                  onChange={(e) =>
                    update((d) => { d.sync.defer_during_recording = e.target.checked; })
                  }
                >
                  До конца активной сессии
                </Checkbox>
              </Labeled>
              <Labeled label="Оффлайн-очередь">
                <Checkbox
                  checked={settings.sync.offline_queue.enabled}
                  onChange={(e) =>
                    update((d) => { d.sync.offline_queue.enabled = e.target.checked; })
                  }
                >
                  Копить выгрузки оффлайн
                </Checkbox>
              </Labeled>
            </Grid>
          </Card>

          <Card>
            <BlockHead numeral="E" title="Целостность" />
            <Grid>
              <Labeled label="Хеш-цепочка">
                <Checkbox
                  checked={settings.integrity.hash_chain}
                  onChange={(e) =>
                    update((d) => { d.integrity.hash_chain = e.target.checked; })
                  }
                >
                  SHA-256 по сегментам, цепочкой
                </Checkbox>
              </Labeled>
              <Labeled label="Журнал событий">
                <Checkbox
                  checked={settings.integrity.event_log}
                  onChange={(e) =>
                    update((d) => { d.integrity.event_log = e.target.checked; })
                  }
                >
                  Старт/пауза/обрыв/восстановление
                </Checkbox>
              </Labeled>
              <Labeled label="ГОСТ ЭЦП (фаза 2)">
                <Checkbox checked={settings.integrity.gost_sign} disabled>
                  Недоступно в v1
                </Checkbox>
              </Labeled>
            </Grid>
          </Card>

          <Card>
            <BlockHead numeral="F" title="Идентичность и авторизация" />
            <Grid>
              <NumField
                label="Кэш-сессия оператора, ч"
                value={settings.auth.operator.cached_session_hours}
                onChange={(v) =>
                  update((d) => { d.auth.operator.cached_session_hours = v; })
                }
              />
            </Grid>
            <Grid>
              <Labeled label="Учётка станции">
                <Checkbox
                  checked={settings.auth.station_identity.required}
                  onChange={(e) =>
                    update((d) => { d.auth.station_identity.required = e.target.checked; })
                  }
                >
                  Требовать учётку зала
                </Checkbox>
              </Labeled>
              <Labeled label="Вход оператора">
                <Checkbox
                  checked={settings.auth.operator.required_to_start}
                  onChange={(e) =>
                    update((d) => { d.auth.operator.required_to_start = e.target.checked; })
                  }
                >
                  Обязателен перед стартом
                </Checkbox>
              </Labeled>
              <Labeled label="Запись переживает токен">
                <Checkbox
                  checked={settings.auth.recording_survives_token_expiry}
                  onChange={(e) =>
                    update((d) => {
                      d.auth.recording_survives_token_expiry = e.target.checked;
                    })
                  }
                >
                  Не прерывать при истечении
                </Checkbox>
              </Labeled>
            </Grid>
          </Card>

          <Card>
            <BlockHead numeral="G" title="Кэш дел" />
            <Grid>
              <Field
                label="Область кэша"
                value={settings.case_cache.scope}
                onChange={(e) => update((d) => { d.case_cache.scope = e.target.value; })}
              />
              <NumField
                label="Свежесть кэша, ч"
                value={settings.case_cache.ttl_hours}
                onChange={(v) => update((d) => { d.case_cache.ttl_hours = v; })}
              />
              <NumField
                label="Лимит записей"
                value={settings.case_cache.max_records}
                onChange={(v) => update((d) => { d.case_cache.max_records = v; })}
              />
            </Grid>
            <Grid>
              <Labeled label="Кэшировать дела">
                <Checkbox
                  checked={settings.case_cache.enabled}
                  onChange={(e) =>
                    update((d) => { d.case_cache.enabled = e.target.checked; })
                  }
                >
                  Для оффлайн-привязки
                </Checkbox>
              </Labeled>
              <Labeled label="Шифровать кэш">
                <Checkbox
                  checked={settings.case_cache.encrypt}
                  onChange={(e) =>
                    update((d) => { d.case_cache.encrypt = e.target.checked; })
                  }
                >
                  Содержит ПДн (ФИО/№)
                </Checkbox>
              </Labeled>
            </Grid>
          </Card>

          <div style={{ display: 'flex', alignItems: 'center', gap: 16 }}>
            <Button
              variant="primary"
              onClick={onSave}
              loading={status.kind === 'saving'}
              disabled={hasErrors}
            >
              Сохранить
            </Button>
            {status.kind === 'saved' && <Tag tone="green">Сохранено</Tag>}
            {hasErrors && <Tag tone="accent">Исправьте ошибки в форме</Tag>}
          </div>
        </>
      )}
    </div>
  );
}

/** Валидация формы: ключ ошибки → сообщение. Пусто — форма валидна. */
function validate(s: Settings): Record<string, string> {
  const e: Record<string, string> = {};
  const positive = (n: number) => Number.isFinite(n) && n > 0;
  if (!positive(s.audio.sample_rate_hz)) e['sample_rate_hz'] = 'Должно быть > 0';
  if (!positive(s.audio.bit_depth)) e['bit_depth'] = 'Должно быть > 0';
  if (!positive(s.audio.channels)) e['channels'] = 'Минимум 1 канал';
  if (!positive(s.recorder.segment_seconds)) e['segment_seconds'] = 'Должно быть > 0';
  if (!positive(s.recorder.flush_interval_ms)) e['flush_interval_ms'] = 'Должно быть > 0';
  if (!positive(s.sync.chunk_size_mb)) e['chunk_size_mb'] = 'Должно быть > 0';
  if (s.reliability.disk_low_threshold_mb <= s.reliability.disk_critical_mb) {
    e['disk_low'] = 'Порог предупреждения должен быть выше критического';
    e['disk_critical'] = 'Критический порог должен быть ниже порога предупреждения';
  }
  if (s.sync.auto_upload && !s.sync.server_base_url) {
    e['server_base_url'] = 'URL сервера обязателен при авто-выгрузке';
  }
  if (s.reliability.mirror.enabled && !s.reliability.mirror.path) {
    e['mirror_path'] = 'Укажите путь для дублирующей дорожки';
  }
  if (s.audio.multichannel.enabled) {
    if (s.audio.tracks.length === 0) {
      e['tracks'] = 'Добавьте хотя бы одну дорожку или выключите многоканал';
    } else if (s.audio.tracks.some((t) => !s.audio.roles.includes(t.role))) {
      e['tracks'] = 'У каждой дорожки должна быть роль из справочника';
    } else if (s.audio.sync.clock_master_track >= s.audio.tracks.length) {
      e['tracks'] = 'Дорожка-мастер клока вне диапазона дорожек';
    }
  }
  return e;
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}

// ── Многоканальная запись по ролям (этап 09) ─────────────────────────────────

/**
 * Секция «Многоканальная запись»: тумблер режима, справочник ролей и карта
 * дорожек «устройство/канал → роль». Аддитивно: по умолчанию выключено (v1 —
 * один канал). Роли уходят в ex_system как вход диаризации W2.11.
 */
function MultichannelCard({
  settings,
  errors,
  devices,
  update,
}: {
  settings: Settings;
  errors: Record<string, string>;
  devices: DeviceInfo[];
  update: (mut: (draft: Settings) => void) => void;
}) {
  const audio = settings.audio;
  const enabled = audio.multichannel.enabled;

  function addTrack() {
    update((d) => {
      const role = d.audio.roles[0] ?? '';
      d.audio.tracks.push({ device: null, channel_index: d.audio.tracks.length, role, label: '' });
    });
  }
  function removeTrack(i: number) {
    update((d) => { d.audio.tracks.splice(i, 1); });
  }
  function patchTrack(i: number, patch: Partial<TrackConfig>) {
    update((d) => { d.audio.tracks[i] = { ...d.audio.tracks[i], ...patch }; });
  }

  return (
    <Card>
      <BlockHead
        numeral="A2"
        title="Многоканальная запись по ролям"
        hint="Фаза 2: N синхронных дорожек, привязанных к ролям (судья, защита, …)"
      />
      <Grid>
        <Labeled label="Режим">
          <Checkbox
            checked={enabled}
            onChange={(e) => update((d) => { d.audio.multichannel.enabled = e.target.checked; })}
          >
            Включить многоканальный захват
          </Checkbox>
        </Labeled>
        <Labeled label="Сведённый мастер">
          <Checkbox
            checked={audio.master_downmix.enabled}
            disabled={!enabled}
            onChange={(e) => update((d) => { d.audio.master_downmix.enabled = e.target.checked; })}
          >
            Микс всех дорожек в один файл
          </Checkbox>
        </Labeled>
      </Grid>

      {enabled && (
        <>
          <Grid>
            <Field
              label="Справочник ролей (через запятую)"
              placeholder="judge, clerk, prosecution, defense, witness, room"
              value={audio.roles.join(', ')}
              onChange={(e) =>
                update((d) => {
                  d.audio.roles = e.target.value
                    .split(',')
                    .map((r) => r.trim())
                    .filter((r) => r.length > 0);
                })
              }
            />
            <LabeledWithTip
              label="Опорная дорожка синхронизации"
              tip="Эталон времени: по частоте дискретизации этой дорожки выравниваются остальные при раздельных устройствах (компенсация дрейфа). На одном многоканальном интерфейсе с общим клоком дрейфа почти нет — параметр не срабатывает."
            >
              <Select
                ariaLabel="Опорная дорожка синхронизации"
                value={String(audio.sync.clock_master_track)}
                onChange={(v) => update((d) => { d.audio.sync.clock_master_track = Number(v); })}
                triggerStyle={selectTriggerStyle}
                options={
                  audio.tracks.length > 0
                    ? audio.tracks.map((t, i) => ({
                        value: String(i),
                        label: `Дорожка ${i + 1} · ${t.label.trim() || t.role}`,
                      }))
                    : [{ value: '0', label: '— сначала добавьте дорожки —' }]
                }
              />
            </LabeledWithTip>
            <NumField
              label="Порог дрейфа, мс"
              value={audio.sync.drift_threshold_ms}
              onChange={(v) => update((d) => { d.audio.sync.drift_threshold_ms = v; })}
            />
            <Labeled label="Компенсация дрейфа">
              <Checkbox
                checked={audio.sync.drift_compensate}
                onChange={(e) => update((d) => { d.audio.sync.drift_compensate = e.target.checked; })}
              >
                Выравнивать раздельные устройства
              </Checkbox>
            </Labeled>
          </Grid>

          <div style={{ marginTop: 16 }}>
            <TracksEditor
              tracks={audio.tracks}
              roles={audio.roles}
              devices={devices}
              error={errors['tracks']}
              onAdd={addTrack}
              onRemove={removeTrack}
              onPatch={patchTrack}
            />
          </div>
        </>
      )}
    </Card>
  );
}

// Нейтральная кнопка на светлой карточке: `secondary` рассчитан на тёмную панель
// (светлый текст), поэтому переопределяем цвет/рамку под бумагу — тот же приём,
// что для кнопок паузы/резюма на экране «Запись».
const NEUTRAL_BTN: CSSProperties = { color: 'var(--ink)', borderColor: 'var(--ink-soft)' };

// Единая высота контролов строки дорожки: и `Field`, и кастомный `Select`
// приводятся к 44px, иначе высота «пляшет» от поля к полю.
const CONTROL_HEIGHT = 44;
const selectTriggerStyle: CSSProperties = {
  minHeight: CONTROL_HEIGHT,
  padding: '11px 36px 11px 12px',
};

/** Таблица дорожек «устройство/канал → роль» с добавлением/удалением. */
function TracksEditor({
  tracks,
  roles,
  devices,
  error,
  onAdd,
  onRemove,
  onPatch,
}: {
  tracks: TrackConfig[];
  roles: string[];
  devices: DeviceInfo[];
  error?: string;
  onAdd: () => void;
  onRemove: (i: number) => void;
  onPatch: (i: number, patch: Partial<TrackConfig>) => void;
}) {
  // Опции устройства: «системное по умолчанию» (null) + перечисленные устройства.
  const deviceOptions = [
    { value: '', label: 'Системное по умолчанию' },
    ...devices.map((d) => ({
      value: d.name,
      label: d.is_default ? `${d.name} · по умолчанию` : d.name,
    })),
  ];

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      <span
        style={{
          fontSize: 11,
          textTransform: 'uppercase',
          letterSpacing: '0.14em',
          color: 'var(--muted)',
          fontWeight: 500,
        }}
      >
        Дорожки · карта «устройство/канал → роль»
      </span>

      {tracks.length === 0 && (
        <Tag tone="accent">Добавьте хотя бы одну дорожку</Tag>
      )}

      {tracks.map((t, i) => (
        <div
          key={i}
          style={{
            display: 'grid',
            // `minmax(0, …fr)`: колонка не раздвигается под содержимое (длинное
            // имя устройства усекается в Select), ширины полей стабильны.
            gridTemplateColumns:
              'minmax(0, 1.6fr) minmax(0, 0.7fr) minmax(0, 1fr) minmax(0, 1.3fr) auto',
            gap: 12,
            alignItems: 'end',
          }}
        >
          <Labeled label="Устройство">
            <Select
              ariaLabel={`Устройство дорожки ${i + 1}`}
              value={t.device ?? ''}
              onChange={(v) => onPatch(i, { device: v === '' ? null : v })}
              options={deviceOptions}
              triggerStyle={selectTriggerStyle}
            />
          </Labeled>
          <Field
            label="Канал"
            type="number"
            min={0}
            value={String(t.channel_index)}
            onChange={(e) => {
              const n = Number(e.target.value);
              if (!Number.isNaN(n)) onPatch(i, { channel_index: n });
            }}
          />
          <Labeled label="Роль">
            <Select
              ariaLabel={`Роль дорожки ${i + 1}`}
              value={t.role}
              onChange={(v) => onPatch(i, { role: v })}
              options={roles.map((r) => ({ value: r, label: r }))}
              triggerStyle={selectTriggerStyle}
            />
          </Labeled>
          <Field
            label="Метка"
            placeholder="Свидетель у трибуны"
            value={t.label}
            onChange={(e) => onPatch(i, { label: e.target.value })}
          />
          <RemoveTrackButton index={i} onClick={() => onRemove(i)} />
        </div>
      ))}

      {error && <Tag tone="accent">{error}</Tag>}

      <div>
        <Button
          variant="secondary"
          style={NEUTRAL_BTN}
          leftIcon={<Icon name="icon-add" size={14} decorative />}
          onClick={onAdd}
        >
          Добавить дорожку
        </Button>
      </div>
    </div>
  );
}

/**
 * Кнопка удаления дорожки — типовая иконка-крест в акцентном цвете дизайна, без
 * рамки/коробки. Выровнена по высоте контролов строки; текст — в
 * `aria-label`/`title`.
 */
function RemoveTrackButton({ index, onClick }: { index: number; onClick: () => void }) {
  const [hover, setHover] = useState(false);
  return (
    <button
      type="button"
      onClick={onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      aria-label={`Удалить дорожку ${index + 1}`}
      title="Удалить дорожку"
      style={{
        height: CONTROL_HEIGHT,
        width: 32,
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'transparent',
        border: 'none',
        padding: 0,
        color: hover ? 'var(--accent-deep)' : 'var(--accent)',
        cursor: 'pointer',
        transition: 'color 120ms ease',
      }}
    >
      <Icon name="icon-close" size={16} decorative />
    </button>
  );
}

// ── Раскладка/обёртки ────────────────────────────────────────────────────────

function SectionTitle({ children }: { children: ReactNode }) {
  return (
    <h2
      style={{
        margin: '4px 0 -4px',
        fontFamily: 'var(--serif)',
        fontWeight: 500,
        fontSize: 15,
        color: 'var(--accent-deep)',
        letterSpacing: '0.01em',
      }}
    >
      {children}
    </h2>
  );
}

function Grid({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        display: 'grid',
        gridTemplateColumns: 'repeat(auto-fit, minmax(240px, 1fr))',
        gap: 16,
        marginTop: 12,
      }}
    >
      {children}
    </div>
  );
}

const labeledCaptionStyle: CSSProperties = {
  fontSize: 11,
  textTransform: 'uppercase',
  letterSpacing: '0.14em',
  color: 'var(--muted)',
  fontWeight: 500,
};

function Labeled({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <span style={labeledCaptionStyle}>{label}</span>
      {children}
    </div>
  );
}

/** Как `Labeled`, но со стандартной иконкой-пояснением «ⓘ» рядом с подписью.
 * `InfoTip` — вне стилизованной подписи, иначе всплывающий текст унаследовал бы
 * `text-transform`/`letter-spacing` и выводился бы капсом. */
function LabeledWithTip({
  label,
  tip,
  children,
}: {
  label: string;
  tip: string;
  children: ReactNode;
}) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
        <span style={labeledCaptionStyle}>{label}</span>
        <InfoTip text={tip} label="Что это?" />
      </span>
      {children}
    </div>
  );
}

function NumField({
  label,
  value,
  error,
  onChange,
}: {
  label: string;
  value: number;
  error?: string;
  onChange: (v: number) => void;
}) {
  return (
    <Field
      label={label}
      type="number"
      value={String(value)}
      error={error}
      onChange={(e) => {
        const n = Number(e.target.value);
        if (!Number.isNaN(n)) onChange(n);
      }}
    />
  );
}
