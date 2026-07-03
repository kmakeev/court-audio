import { useEffect, useState } from 'react';
import { BlockHead, Button, Card, Field, screenStackStyle, Tag } from '../design';
import { listAudioDevices, type DeviceInfo } from '../lib/core';
import {
  adminLock,
  adminStatus,
  adminUnlock,
  exportStationProfile,
  getSettings,
  getSettingsAudit,
  importStationProfile,
  saveSettings,
  type AdminStatus,
  type SaveOutcome,
  type Settings,
  type SettingsAuditRecord,
} from '../lib/settings';
import {
  AdminSections,
  describeError,
  NEUTRAL_BTN,
  SectionTitle,
  validateAdmin,
} from './settings-common';

// Экран «Администрирование» (этап 10.4). Инфраструктура и безопасность станции.
// Права администратора в v1 — валидный админ-PIN (оффлайн-фолбэк). Пока доступ
// не разблокирован — параметры/профиль/журнал **скрыты** (никаких операций без
// PIN). Опасные изменения (сервер/шифрование/ретеншн) требуют подтверждения; все
// изменения — в журнале. Профиль станции (полный Settings без секретов) —
// тиражирование на залы; импорт только администратором, с журналом.

// Сколько последних записей журнала показывать (презентация, не бизнес-логика).
const AUDIT_LIMIT = 100;

type Status =
  | { kind: 'idle' }
  | { kind: 'saving' }
  | { kind: 'saved' }
  | { kind: 'error'; message: string };

type Pending = { dangerous: string[]; confirm: () => Promise<void> } | null;

export function AdministrationScreen() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [admin, setAdmin] = useState<AdminStatus | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [audit, setAudit] = useState<SettingsAuditRecord[]>([]);
  const [pin, setPin] = useState('');
  const [status, setStatus] = useState<Status>({ kind: 'idle' });
  const [pending, setPending] = useState<Pending>(null);
  const [profileText, setProfileText] = useState('');
  const [profileMsg, setProfileMsg] = useState<string | null>(null);

  useEffect(() => {
    getSettings().then(setSettings).catch(() => {});
    adminStatus().then(setAdmin).catch(() => {});
    listAudioDevices().then(setDevices).catch(() => setDevices([]));
    void reloadAudit();
  }, []);

  function reloadAudit() {
    return getSettingsAudit(AUDIT_LIMIT)
      .then(setAudit)
      .catch(() => setAudit([]));
  }

  // Раздел открыт, если админ-доступ разблокирован **или** гейт админ-PIN
  // выключен (`admin.pin.required = false`) — тогда PIN не нужен.
  const open = admin ? admin.unlocked || !admin.required : false;

  function update(mut: (draft: Settings) => void) {
    setSettings((prev) => {
      if (!prev) return prev;
      const next = structuredClone(prev);
      mut(next);
      return next;
    });
    setStatus({ kind: 'idle' });
  }

  const errors = settings ? validateAdmin(settings) : {};
  const hasErrors = Object.keys(errors).length > 0;

  async function onUnlock() {
    try {
      const s = await adminUnlock(pin);
      setAdmin(s);
      setPin('');
      setStatus({ kind: 'idle' });
    } catch (e) {
      setStatus({ kind: 'error', message: describeError(e) });
    }
  }

  async function onLock() {
    try {
      setAdmin(await adminLock());
      setPending(null);
      setProfileText('');
      setProfileMsg(null);
    } catch {
      /* игнорируем — блокировка best-effort */
    }
  }

  // Общий прогон сохранения/импорта с обработкой подтверждения опасных изменений.
  async function runOutcome(action: (confirm: boolean) => Promise<SaveOutcome>) {
    setStatus({ kind: 'saving' });
    setProfileMsg(null);
    try {
      const out = await action(false);
      if (out.kind === 'needs_confirmation') {
        setStatus({ kind: 'idle' });
        setPending({
          dangerous: out.dangerous,
          confirm: async () => {
            setStatus({ kind: 'saving' });
            try {
              await action(true);
              setPending(null);
              await afterApply();
            } catch (e) {
              setPending(null);
              setStatus({ kind: 'error', message: describeError(e) });
            }
          },
        });
        return;
      }
      await afterApply();
    } catch (e) {
      setStatus({ kind: 'error', message: describeError(e) });
    }
  }

  async function afterApply() {
    setStatus({ kind: 'saved' });
    // Импорт мог заменить весь конфиг — перечитываем.
    const s = await getSettings().catch(() => null);
    if (s) setSettings(s);
    await reloadAudit();
  }

  function onSave() {
    if (!settings || hasErrors) return;
    void runOutcome((confirm) => saveSettings(settings, confirm));
  }

  async function onExport() {
    try {
      const json = await exportStationProfile();
      setProfileText(json);
      setProfileMsg('Профиль выгружен ниже — скопируйте JSON.');
    } catch (e) {
      setProfileMsg(describeError(e));
    }
  }

  function onImport() {
    if (!profileText.trim()) {
      setProfileMsg('Вставьте JSON профиля станции.');
      return;
    }
    void runOutcome((confirm) => importStationProfile(profileText, confirm));
  }

  return (
    <div style={screenStackStyle(760)}>
      <Card>
        <BlockHead
          numeral="04"
          title="Администрирование станции"
          hint="Инфраструктура и безопасность. Доступно после разблокировки админ-доступа."
        />
        <AdminAccessBar
          admin={admin}
          pin={pin}
          onPin={setPin}
          onUnlock={() => void onUnlock()}
          onLock={() => void onLock()}
        />
        {status.kind === 'error' && (
          <div style={{ marginTop: 8 }}>
            <Tag tone="accent">Ошибка: {status.message}</Tag>
          </div>
        )}
      </Card>

      {/* Пока не открыто — никаких операций/параметров не показываем. */}
      {admin && !open && (
        <Card>
          <span style={{ fontSize: 14, color: 'var(--muted)' }}>
            {admin.provisioned === false
              ? 'Настройки станции недоступны: админ-PIN не задан при развёртывании (env COURT_AUDIO_ADMIN_PIN).'
              : 'Разблокируйте админ-доступ по PIN, чтобы просматривать и изменять настройки станции.'}
          </span>
        </Card>
      )}

      {open && settings && (
        <>
          {pending && (
            <ConfirmDanger
              dangerous={pending.dangerous}
              onConfirm={() => void pending.confirm()}
              onCancel={() => setPending(null)}
            />
          )}

          <SectionTitle>Параметры станции</SectionTitle>
          <AdminSections
            settings={settings}
            errors={errors}
            devices={devices}
            update={update}
            disabled={false}
          />

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

          <SectionTitle>Профиль станции</SectionTitle>
          <Card>
            <BlockHead
              numeral="P"
              title="Экспорт / импорт профиля"
              hint="Полная конфигурация станции в JSON (без секретов) — тиражирование на залы и восстановление. Импорт журналируется."
            />
            <div style={{ display: 'flex', gap: 12, marginTop: 12, flexWrap: 'wrap' }}>
              <Button variant="secondary" style={NEUTRAL_BTN} onClick={() => void onExport()}>
                Экспортировать профиль
              </Button>
              <Button variant="secondary" style={NEUTRAL_BTN} onClick={onImport}>
                Импортировать профиль
              </Button>
            </div>
            <textarea
              aria-label="Профиль станции (JSON)"
              value={profileText}
              onChange={(e) => setProfileText(e.target.value)}
              placeholder="JSON профиля станции…"
              spellCheck={false}
              style={{
                width: '100%',
                minHeight: 160,
                marginTop: 12,
                fontFamily: 'var(--mono, monospace)',
                fontSize: 12,
                padding: 12,
                border: '1px solid var(--hairline)',
                borderRadius: 6,
                background: 'var(--paper-soft)',
                color: 'var(--ink)',
                resize: 'vertical',
                boxSizing: 'border-box',
              }}
            />
            {profileMsg && (
              <div style={{ marginTop: 8 }}>
                <Tag tone="accent">{profileMsg}</Tag>
              </div>
            )}
          </Card>

          <SectionTitle>Журнал изменений настроек</SectionTitle>
          <AuditLog records={audit} />
        </>
      )}
    </div>
  );
}

// ── Панель админ-доступа ──────────────────────────────────────────────────────

function AdminAccessBar({
  admin,
  pin,
  onPin,
  onUnlock,
  onLock,
}: {
  admin: AdminStatus | null;
  pin: string;
  onPin: (v: string) => void;
  onUnlock: () => void;
  onLock: () => void;
}) {
  if (!admin) return null;

  // Гейт админ-PIN выключен политикой — раздел открыт без PIN.
  if (!admin.required) {
    return (
      <div style={{ marginTop: 12 }}>
        <Tag tone="gold">
          Гейт админ-PIN отключён — настройки станции открыты без ввода PIN.
        </Tag>
      </div>
    );
  }

  if (!admin.provisioned) {
    return (
      <div style={{ marginTop: 12 }}>
        <Tag tone="accent">
          Админ-PIN не задан при развёртывании — задайте env COURT_AUDIO_ADMIN_PIN.
          Изменение админ-настроек недоступно.
        </Tag>
      </div>
    );
  }

  if (admin.unlocked) {
    return (
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 12 }}>
        <Tag tone="green">Админ-доступ разблокирован</Tag>
        <Button variant="secondary" style={NEUTRAL_BTN} onClick={onLock}>
          Заблокировать
        </Button>
      </div>
    );
  }

  return (
    <div style={{ display: 'flex', alignItems: 'flex-end', gap: 12, marginTop: 12, flexWrap: 'wrap' }}>
      <div style={{ minWidth: 220, flex: '0 1 260px' }}>
        <Field
          label="Админ-PIN"
          type="password"
          value={pin}
          placeholder="••••"
          onChange={(e) => onPin(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') onUnlock();
          }}
        />
      </div>
      <Button variant="primary" onClick={onUnlock} disabled={pin.trim() === ''}>
        Разблокировать
      </Button>
    </div>
  );
}

// ── Диалог подтверждения опасных изменений ────────────────────────────────────

function ConfirmDanger({
  dangerous,
  onConfirm,
  onCancel,
}: {
  dangerous: string[];
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <Card>
      <BlockHead numeral="!" title="Подтвердите опасные изменения" />
      <p style={{ fontSize: 14, color: 'var(--ink)', marginTop: 8 }}>
        Эти изменения затрагивают безопасность/целостность станции:
      </p>
      <ul style={{ margin: '8px 0', paddingLeft: 20, fontSize: 14, color: 'var(--ink)' }}>
        {dangerous.map((d) => (
          <li key={d}>{d}</li>
        ))}
      </ul>
      <div style={{ display: 'flex', gap: 12, marginTop: 8 }}>
        <Button variant="primary" onClick={onConfirm}>
          Подтвердить и сохранить
        </Button>
        <Button variant="secondary" style={NEUTRAL_BTN} onClick={onCancel}>
          Отмена
        </Button>
      </div>
    </Card>
  );
}

// ── Журнал изменений ──────────────────────────────────────────────────────────

const SOURCE_LABELS: Record<string, string> = {
  manual: 'Сохранение',
  import: 'Импорт профиля',
};

function AuditLog({ records }: { records: SettingsAuditRecord[] }) {
  if (records.length === 0) {
    return (
      <Card>
        <span style={{ fontSize: 14, color: 'var(--muted)' }}>
          Изменений настроек пока не было.
        </span>
      </Card>
    );
  }
  return (
    <Card>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
        {records.map((r) => (
          <div
            key={r.seq}
            style={{
              display: 'flex',
              flexDirection: 'column',
              gap: 4,
              paddingBottom: 12,
              borderBottom: '1px solid var(--hairline)',
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
              <span style={{ fontSize: 13, fontWeight: 500, color: 'var(--ink)' }}>
                {new Date(r.at_unix_ms).toLocaleString()}
              </span>
              <span style={{ fontSize: 12, color: 'var(--muted)' }}>
                {SOURCE_LABELS[r.source] ?? r.source} ·{' '}
                {r.actor_operator_id ? `оператор #${r.actor_operator_id}` : 'станция'}
              </span>
              {r.dangerous && <Tag tone="accent">Опасное</Tag>}
            </div>
            <div style={{ fontSize: 12, color: 'var(--ink-soft)' }}>
              {r.changes.map((c) => c.path).join(', ') || '—'}
            </div>
          </div>
        ))}
      </div>
    </Card>
  );
}
