import { useEffect, useState, type ReactNode } from 'react';
import { BlockHead, Button, Card, Checkbox, Field, Select, Tag } from '../design';
import {
  getSettings,
  saveSettings,
  type RetentionMode,
  type Settings,
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

  useEffect(() => {
    getSettings()
      .then((s) => {
        setSettings(s);
        setStatus({ kind: 'ready' });
      })
      .catch((e: unknown) => setStatus({ kind: 'error', message: describeError(e) }));
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
  return e;
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
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

function Labeled({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <span
        style={{
          fontSize: 11,
          textTransform: 'uppercase',
          letterSpacing: '0.14em',
          color: 'var(--muted)',
          fontWeight: 500,
        }}
      >
        {label}
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
