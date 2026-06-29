import { useEffect, useState, type CSSProperties, type ReactNode } from 'react';
import { BlockHead, Button, Card, Tag } from '../design';
import {
  getSettings,
  saveSettings,
  type RetentionMode,
  type Settings,
} from '../lib/settings';

// Экран «Настройки» этапа 00: читает модель `Settings` из ядра, показывает
// ключевые поля и персистит обратно. Без побочных эффектов на запись —
// значения по умолчанию задаёт Rust по реестру docs/configuration.md.

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
      .catch((e: unknown) =>
        setStatus({ kind: 'error', message: describeError(e) })
      );
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

  async function onSave() {
    if (!settings) return;
    setStatus({ kind: 'saving' });
    try {
      await saveSettings(settings);
      setStatus({ kind: 'saved' });
    } catch (e) {
      setStatus({ kind: 'error', message: describeError(e) });
    }
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 720 }}>
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
          <Card>
            <BlockHead numeral="A" title="Аудио" />
            <FieldGrid>
              <Field label="Устройство ввода" hint="Пусто — системное по умолчанию">
                <TextInput
                  value={settings.audio.device ?? ''}
                  placeholder="Системное по умолчанию"
                  onChange={(v) =>
                    update((d) => {
                      d.audio.device = v.trim() === '' ? null : v;
                    })
                  }
                />
              </Field>
              <Field label="Частота, Гц">
                <NumberInput
                  value={settings.audio.sample_rate_hz}
                  onChange={(v) => update((d) => { d.audio.sample_rate_hz = v; })}
                />
              </Field>
              <Field label="Разрядность, бит">
                <NumberInput
                  value={settings.audio.bit_depth}
                  onChange={(v) => update((d) => { d.audio.bit_depth = v; })}
                />
              </Field>
              <Field label="Каналы">
                <NumberInput
                  value={settings.audio.channels}
                  onChange={(v) => update((d) => { d.audio.channels = v; })}
                />
              </Field>
            </FieldGrid>
          </Card>

          <Card>
            <BlockHead numeral="B" title="Хранилище и ретеншн" />
            <FieldGrid>
              <Field label="Корень хранилища" hint="Пусто — <data-dir>/recordings">
                <TextInput
                  value={settings.storage.root_path ?? ''}
                  placeholder="<data-dir>/recordings"
                  onChange={(v) =>
                    update((d) => {
                      d.storage.root_path = v.trim() === '' ? null : v;
                    })
                  }
                />
              </Field>
              <Field label="Политика удаления">
                <select
                  value={settings.retention.mode}
                  onChange={(e) =>
                    update((d) => {
                      d.retention.mode = e.target.value as RetentionMode;
                    })
                  }
                  style={inputStyle}
                >
                  {(Object.keys(RETENTION_LABELS) as RetentionMode[]).map((m) => (
                    <option key={m} value={m}>
                      {RETENTION_LABELS[m]}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="Буферное окно, ч">
                <NumberInput
                  value={settings.retention.safety_window_hours}
                  onChange={(v) =>
                    update((d) => { d.retention.safety_window_hours = v; })
                  }
                />
              </Field>
              <Field label="Шифрование at-rest">
                <Checkbox
                  checked={settings.storage.encrypt_at_rest}
                  onChange={(c) =>
                    update((d) => { d.storage.encrypt_at_rest = c; })
                  }
                />
              </Field>
            </FieldGrid>
          </Card>

          <Card>
            <BlockHead numeral="C" title="Сеть и выгрузка" />
            <FieldGrid>
              <Field label="URL сервера ex_system" hint="Обязателен для выгрузки">
                <TextInput
                  value={settings.sync.server_base_url ?? ''}
                  placeholder="https://…"
                  onChange={(v) =>
                    update((d) => {
                      d.sync.server_base_url = v.trim() === '' ? null : v;
                    })
                  }
                />
              </Field>
              <Field label="Размер чанка, МБ">
                <NumberInput
                  value={settings.sync.chunk_size_mb}
                  onChange={(v) => update((d) => { d.sync.chunk_size_mb = v; })}
                />
              </Field>
              <Field label="Авто-выгрузка">
                <Checkbox
                  checked={settings.sync.auto_upload}
                  onChange={(c) => update((d) => { d.sync.auto_upload = c; })}
                />
              </Field>
              <Field label="Оффлайн-очередь">
                <Checkbox
                  checked={settings.sync.offline_queue.enabled}
                  onChange={(c) =>
                    update((d) => { d.sync.offline_queue.enabled = c; })
                  }
                />
              </Field>
            </FieldGrid>
          </Card>

          <div style={{ display: 'flex', alignItems: 'center', gap: 16 }}>
            <Button
              variant="primary"
              onClick={onSave}
              loading={status.kind === 'saving'}
            >
              Сохранить
            </Button>
            {status.kind === 'saved' && <Tag tone="green">Сохранено</Tag>}
          </div>
        </>
      )}
    </div>
  );
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}

// ── Мелкие поля формы на токенах PravoUI ────────────────────────────────────

const inputStyle: CSSProperties = {
  width: '100%',
  height: 38,
  padding: '0 10px',
  background: 'var(--paper-elev)',
  border: '1px solid var(--hairline)',
  color: 'var(--ink)',
  fontFamily: 'var(--sans)',
  fontSize: 13,
};

function FieldGrid({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        display: 'grid',
        gridTemplateColumns: 'repeat(auto-fit, minmax(260px, 1fr))',
        gap: 16,
        marginTop: 12,
      }}
    >
      {children}
    </div>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      <span style={{ fontSize: 12, fontWeight: 500, color: 'var(--ink-soft)' }}>
        {label}
      </span>
      {children}
      {hint && <span style={{ fontSize: 11, color: 'var(--muted)' }}>{hint}</span>}
    </label>
  );
}

function TextInput({
  value,
  placeholder,
  onChange,
}: {
  value: string;
  placeholder?: string;
  onChange: (v: string) => void;
}) {
  return (
    <input
      type="text"
      value={value}
      placeholder={placeholder}
      onChange={(e) => onChange(e.target.value)}
      style={inputStyle}
    />
  );
}

function NumberInput({
  value,
  onChange,
}: {
  value: number;
  onChange: (v: number) => void;
}) {
  return (
    <input
      type="number"
      value={value}
      onChange={(e) => {
        const n = Number(e.target.value);
        if (!Number.isNaN(n)) onChange(n);
      }}
      className="num"
      style={inputStyle}
    />
  );
}

function Checkbox({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (c: boolean) => void;
}) {
  return (
    <input
      type="checkbox"
      checked={checked}
      onChange={(e) => onChange(e.target.checked)}
      style={{ width: 18, height: 18, accentColor: 'var(--accent)' }}
    />
  );
}
