import { useEffect, useState } from 'react';
import { BlockHead, Button, Card, screenStackStyle, Select, Tag } from '../design';
import { listAudioDevices, type DeviceInfo } from '../lib/core';
import { getSettings, saveSettings, type Settings } from '../lib/settings';
import {
  describeError,
  Grid,
  LabeledWithTip,
  ListField,
  NumField,
  parseNumberList,
  SectionTitle,
  splitList,
  validateOperator,
} from './settings-common';

// Экран «Настройки» (оператор, этап 10.4). Только **оператор-скоуп** — устройство,
// справочники меток/ролей, удобства проигрывателя (реестр docs/configuration.md →
// «Область доступа»). Инфраструктура и безопасность станции — на отдельном экране
// «Администрирование» (за админ-PIN); гейт — в ядре (ipc::settings_gate).

type Status =
  | { kind: 'loading' }
  | { kind: 'ready' }
  | { kind: 'saving' }
  | { kind: 'saved' }
  | { kind: 'error'; message: string };

/**
 * Опции выпадающего списка устройства ввода: «системное по умолчанию» + все
 * перечисленные устройства. Если сохранённое устройство сейчас не в списке
 * (отключено/переименовано) — добавляем его отдельной опцией, чтобы выбор не
 * потерялся молча.
 */
function deviceOptions(devices: DeviceInfo[], current: string | null) {
  const opts = [
    { value: '', label: 'Системное по умолчанию' },
    ...devices.map((d) => ({
      value: d.name,
      label: d.is_default ? `${d.name} · по умолчанию` : d.name,
    })),
  ];
  if (current && !devices.some((d) => d.name === current)) {
    opts.push({ value: current, label: `${current} · не подключено` });
  }
  return opts;
}

export function SettingsScreen() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [status, setStatus] = useState<Status>({ kind: 'loading' });
  // Список устройств ввода — для выпадающего выбора источника записи.
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

  const errors = settings ? validateOperator(settings) : {};
  const hasErrors = Object.keys(errors).length > 0;
  // Многоканал активен, когда администратор включил режим И задал карту дорожек
  // (то же условие, что в ядре — ipc/audio_cmds::start_capture). В этом режиме
  // единый выбор устройства ввода не участвует в захвате: источник каждой
  // дорожки берётся из карты (admin-скоуп). Поле отключаем, чтобы не вводить в
  // заблуждение (R-008, этап 13.2).
  const multichannelActive =
    !!settings && settings.audio.multichannel.enabled && settings.audio.tracks.length > 0;

  async function onSave() {
    if (!settings || hasErrors) return;
    setStatus({ kind: 'saving' });
    try {
      // Оператор-скоуп изменения ядро не считает админскими/опасными — Saved.
      await saveSettings(settings, false);
      setStatus({ kind: 'saved' });
    } catch (e) {
      setStatus({ kind: 'error', message: describeError(e) });
    }
  }

  return (
    <div style={screenStackStyle(760)}>
      <Card>
        <BlockHead
          numeral="03"
          title="Настройки станции"
          hint="Рабочие параметры оператора. Инфраструктуру и безопасность меняет администратор."
        />
        {status.kind === 'error' && (
          <div style={{ marginTop: 8 }}>
            <Tag tone="accent">Ошибка: {status.message}</Tag>
          </div>
        )}
      </Card>

      {settings && (
        <>
          <SectionTitle>Оператор · рабочие параметры</SectionTitle>

          <Card>
            <BlockHead numeral="A" title="Аудио · устройство" />
            <Grid>
              <LabeledWithTip
                label="Устройство ввода"
                tip={
                  multichannelActive
                    ? 'Включена многоканальная запись: источник каждой дорожки задаёт карта дорожек в разделе «Многоканальная запись» экрана «Администрирование». Единое устройство ввода в этом режиме не используется.'
                    : 'С какого микрофона/интерфейса вести запись. «Системное по умолчанию» — устройство, выбранное в ОС. Если сохранённое устройство сейчас не подключено, оно помечено «не подключено» — верните его или выберите другое.'
                }
              >
                <Select
                  ariaLabel="Устройство ввода"
                  value={settings.audio.device ?? ''}
                  disabled={multichannelActive}
                  onChange={(v) =>
                    update((d) => { d.audio.device = v === '' ? null : v; })
                  }
                  options={deviceOptions(devices, settings.audio.device)}
                />
                {multichannelActive && (
                  <span style={{ fontSize: 12, color: 'var(--muted)', lineHeight: 1.4 }}>
                    Многоканальный режим включён администратором — устройства дорожек
                    настраиваются в «Администрировании». Единый выбор устройства здесь
                    отключён.
                  </span>
                )}
              </LabeledWithTip>
            </Grid>
          </Card>

          <Card>
            <BlockHead
              numeral="A1"
              title="Разметка: метки и роли"
              hint="Справочники живой разметки заседания. Доступны и без многоканала — оператор ведёт их заранее."
            />
            <Grid>
              <ListField
                label="Категории закладок (через запятую)"
                placeholder="Закладка, Инцидент, Перерыв, Прочее"
                value={settings.markers.categories}
                parse={splitList}
                onCommit={(v) => update((d) => { d.markers.categories = v; })}
              />
              <ListField
                label="Роли говорящих (через запятую)"
                placeholder="judge, clerk, prosecution, defense, witness, room"
                value={settings.audio.roles}
                parse={splitList}
                onCommit={(v) => update((d) => { d.audio.roles = v; })}
              />
            </Grid>
          </Card>

          <Card>
            <BlockHead
              numeral="A2"
              title="Проигрыватель"
              hint="Удобства встроенного проигрывателя сессий (этап 10.1)."
            />
            <Grid>
              <NumField
                label="Шаг перемотки, с"
                value={settings.player.seek_step_seconds}
                error={errors['seek_step']}
                onChange={(v) => update((d) => { d.player.seek_step_seconds = v; })}
              />
              <NumField
                label="Частота позиции, Гц"
                value={settings.player.position_update_hz}
                error={errors['position_hz']}
                onChange={(v) => update((d) => { d.player.position_update_hz = v; })}
              />
              <ListField
                label="Скорости (через запятую)"
                placeholder="0.5, 0.75, 1.0, 1.25, 1.5, 2.0"
                value={settings.player.playback_rates}
                parse={parseNumberList}
                error={errors['playback_rates']}
                onCommit={(v) => update((d) => { d.player.playback_rates = v; })}
              />
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
