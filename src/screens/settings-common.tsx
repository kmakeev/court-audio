import { useState, type CSSProperties, type ReactNode } from 'react';
import { BlockHead, Button, Card, Checkbox, Field, Icon, InfoTip, Select, Tag } from '../design';
import type { DeviceInfo } from '../lib/core';
import type { RetentionMode, Settings, TrackConfig } from '../lib/settings';

// Общие для экранов «Настройки» (оператор) и «Администрирование» примитивы
// раскладки + админ-секции реестра. Разграничение оператор/админ — реестр
// docs/configuration.md → «Область доступа». Гейт на сохранении — в ядре
// (ipc::settings_gate); здесь `disabled` лишь делает админ-секцию read-only для
// прозрачности (оператор видит конфигурацию станции, но не меняет её).

export type UpdateFn = (mut: (draft: Settings) => void) => void;

export const RETENTION_LABELS: Record<RetentionMode, string> = {
  until_confirmed_plus_window: 'До подтверждения + буферное окно',
  delete_on_confirm: 'Удалять сразу по подтверждению',
  manual: 'До ручного удаления',
};

/** Валидация админ-секции: ключ ошибки → сообщение. Пусто — секция валидна. */
export function validateAdmin(s: Settings): Record<string, string> {
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
  if (s.admin.pin.min_length <= 0) e['admin_pin_min'] = 'Минимальная длина PIN > 0';
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

/** Валидация оператор-секции (устройство/справочники/плеер). */
export function validateOperator(s: Settings): Record<string, string> {
  const e: Record<string, string> = {};
  const positive = (n: number) => Number.isFinite(n) && n > 0;
  if (!positive(s.player.seek_step_seconds)) e['seek_step'] = 'Шаг перемотки > 0';
  if (!positive(s.player.position_update_hz)) e['position_hz'] = 'Частота позиции > 0';
  if (s.player.playback_rates.length === 0 || s.player.playback_rates.some((r) => !positive(r))) {
    e['playback_rates'] = 'Скорости — положительные числа через запятую';
  }
  return e;
}

export function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}

/** Разобрать список «через запятую»: обрезать пробелы, отбросить пустые. */
export function splitList(raw: string): string[] {
  return raw
    .split(',')
    .map((r) => r.trim())
    .filter((r) => r.length > 0);
}

/** Разобрать список чисел «через запятую» (скорости воспроизведения). */
export function parseNumberList(raw: string): number[] {
  return splitList(raw)
    .map((r) => Number(r))
    .filter((n) => Number.isFinite(n));
}

// ── Админ-секции реестра ──────────────────────────────────────────────────────

/**
 * Все админ-секции настроек (реестр docs/configuration.md → «Область доступа»).
 * `disabled` делает их read-only: на экране «Настройки» оператор видит
 * конфигурацию, но правит её только на «Администрировании» после разблокировки.
 */
export function AdminSections({
  settings,
  errors,
  devices,
  update,
  disabled,
}: {
  settings: Settings;
  errors: Record<string, string>;
  devices: DeviceInfo[];
  update: UpdateFn;
  disabled: boolean;
}) {
  return (
    <>
      <Card>
        <BlockHead numeral="A1" title="Аудио · качество и формат" />
        <Grid>
          <NumField
            label="Частота, Гц"
            value={settings.audio.sample_rate_hz}
            error={errors['sample_rate_hz']}
            disabled={disabled}
            tip={hint('Частота дискретизации мастер-записи. Если устройство её не поддерживает — запись идёт на родной частоте устройства без пересчёта. 44100 Гц — стандарт; для одной речи достаточно.')}
            onChange={(v) => update((d) => { d.audio.sample_rate_hz = v; })}
          />
          <NumField
            label="Разрядность, бит"
            value={settings.audio.bit_depth}
            error={errors['bit_depth']}
            disabled={disabled}
            tip={hint('Разрядность отсчёта PCM. 16 бит — стандарт для речи; больше — точнее тихие места, но крупнее файлы.')}
            onChange={(v) => update((d) => { d.audio.bit_depth = v; })}
          />
          <NumField
            label="Каналы"
            value={settings.audio.channels}
            error={errors['channels']}
            disabled={disabled}
            tip={hint('Число каналов записи. 1 — моно (один качественный канал, рекомендуется). Запись по ролям на разные дорожки — в разделе «Многоканальная запись» ниже.')}
            onChange={(v) => update((d) => { d.audio.channels = v; })}
          />
        </Grid>
        <Grid>
          <LabeledWithTip
            label="Архивная копия (FLAC)"
            tip="Дополнительная сжатая копия записи в FLAC поверх мастер-WAV — компактнее для архива, без потери качества. Основную запись не заменяет."
          >
            <Checkbox
              checked={settings.audio.archive_copy.enabled}
              disabled={disabled}
              onChange={(e) => update((d) => { d.audio.archive_copy.enabled = e.target.checked; })}
            >
              Включить сжатую копию для архива
            </Checkbox>
          </LabeledWithTip>
        </Grid>
      </Card>

      <MultichannelCard
        settings={settings}
        errors={errors}
        devices={devices}
        update={update}
        disabled={disabled}
      />

      <Card>
        <BlockHead numeral="B" title="Запись и надёжность" />
        <Grid>
          <NumField
            label="Длина сегмента, с"
            value={settings.recorder.segment_seconds}
            error={errors['segment_seconds']}
            disabled={disabled}
            tip={hint('Запись пишется короткими сегментами. При внезапном отключении питания теряется максимум один незавершённый сегмент. Это же — единица подсчёта хеша целостности.')}
            onChange={(v) => update((d) => { d.recorder.segment_seconds = v; })}
          />
          <NumField
            label="Интервал fsync, мс"
            value={settings.recorder.flush_interval_ms}
            error={errors['flush_interval_ms']}
            disabled={disabled}
            tip={hint('Как часто данные принудительно сбрасываются из памяти на диск. Определяет максимальную потерю звука при внезапном обесточивании: меньше значение — меньше потеря, но выше нагрузка на диск.')}
            onChange={(v) => update((d) => { d.recorder.flush_interval_ms = v; })}
          />
          <NumField
            label="Макс. длительность, ч"
            value={settings.recorder.max_session_hours}
            disabled={disabled}
            tip={hint('Предохранитель от «бесконечной» сессии: по достижении — предупреждение/авто-разбиение записи.')}
            onChange={(v) => update((d) => { d.recorder.max_session_hours = v; })}
          />
          <NumField
            label="Watchdog таймаут, мс"
            value={settings.reliability.watchdog_timeout_ms}
            disabled={disabled}
            tip={hint('Сторожевой таймер: если поток записи не подаёт признаков жизни дольше этого времени — запись автоматически перезапускается.')}
            onChange={(v) => update((d) => { d.reliability.watchdog_timeout_ms = v; })}
          />
          <NumField
            label="Порог «мало места», МБ"
            value={settings.reliability.disk_low_threshold_mb}
            error={errors['disk_low']}
            disabled={disabled}
            tip={hint('Когда свободного места на диске остаётся меньше этого — оператор получает предупреждение. Должен быть выше критического порога.')}
            onChange={(v) => update((d) => { d.reliability.disk_low_threshold_mb = v; })}
          />
          <NumField
            label="Критич. порог места, МБ"
            value={settings.reliability.disk_critical_mb}
            error={errors['disk_critical']}
            disabled={disabled}
            tip={hint('Аварийный порог: при достижении станция выполняет защитные действия — корректно останавливает запись без потери уже сохранённого.')}
            onChange={(v) => update((d) => { d.reliability.disk_critical_mb = v; })}
          />
        </Grid>
        <Grid>
          <LabeledWithTip
            label="Авто-возобновление"
            tip="Если микрофон/устройство пропало и затем вернулось — запись сама продолжается, без действий оператора."
          >
            <Checkbox
              checked={settings.reliability.device_reconnect.auto_resume}
              disabled={disabled}
              onChange={(e) =>
                update((d) => { d.reliability.device_reconnect.auto_resume = e.target.checked; })
              }
            >
              При возврате устройства
            </Checkbox>
          </LabeledWithTip>
          <LabeledWithTip
            label="Дублирующая дорожка"
            tip="Параллельно пишет копию сегментов на второй носитель — страховка на случай отказа основного диска. Путь к носителю задаётся ниже."
          >
            <Checkbox
              checked={settings.reliability.mirror.enabled}
              disabled={disabled}
              onChange={(e) => update((d) => { d.reliability.mirror.enabled = e.target.checked; })}
            >
              Зеркало на второй носитель
            </Checkbox>
          </LabeledWithTip>
          {settings.reliability.mirror.enabled && (
            <Field
              label="Путь зеркала"
              placeholder="/mnt/backup/recordings"
              value={settings.reliability.mirror.path ?? ''}
              error={errors['mirror_path']}
              disabled={disabled}
              onChange={(e) =>
                update((d) => {
                  d.reliability.mirror.path = e.target.value.trim() === '' ? null : e.target.value;
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
            disabled={disabled}
            tip={hint('Каталог, где станция хранит записи локально. Пусто — системный каталог данных приложения. Меняйте только на надёжный локальный диск с достаточным объёмом.')}
            onChange={(e) =>
              update((d) => {
                d.storage.root_path = e.target.value.trim() === '' ? null : e.target.value;
              })
            }
          />
          <LabeledWithTip
            label="Политика удаления"
            tip="Когда удалять локальную копию записи: «до подтверждения + окно» — держать до подтверждения сервером и ещё буферное время; «сразу по подтверждению» — минимизация ПДн; «до ручного удаления» — не удалять автоматически."
          >
            <Select
              ariaLabel="Политика удаления"
              value={settings.retention.mode}
              disabled={disabled}
              onChange={(v) => update((d) => { d.retention.mode = v as RetentionMode; })}
              options={(Object.keys(RETENTION_LABELS) as RetentionMode[]).map((m) => ({
                value: m,
                label: RETENTION_LABELS[m],
              }))}
            />
          </LabeledWithTip>
          <NumField
            label="Буферное окно, ч"
            value={settings.retention.safety_window_hours}
            disabled={disabled}
            tip={hint('Сколько держать локальную копию после серверного подтверждения, прежде чем удалить. Запас на случай, если запись понадобится повторно.')}
            onChange={(v) => update((d) => { d.retention.safety_window_hours = v; })}
          />
        </Grid>
        <Grid>
          <LabeledWithTip
            label="Шифрование at-rest"
            tip="Записи на диске шифруются AES-256-GCM посегментно. Ключ — станционный (из парольной фразы/хранилища ОС), в настройках не хранится. Отключать не рекомендуется: на диск лягут открытые ПДн."
          >
            <Checkbox
              checked={settings.storage.encrypt_at_rest}
              disabled={disabled}
              onChange={(e) => update((d) => { d.storage.encrypt_at_rest = e.target.checked; })}
            >
              AES-256-GCM на сегмент
            </Checkbox>
          </LabeledWithTip>
          <LabeledWithTip
            label="Удалять только после серверной проверки"
            tip="Локальную копию удаляем лишь после того, как сервер подтвердил целостность выгруженной записи. Защита от потери данных при сбое выгрузки."
          >
            <Checkbox
              checked={settings.retention.require_integrity_verified}
              disabled={disabled}
              onChange={(e) =>
                update((d) => { d.retention.require_integrity_verified = e.target.checked; })
              }
            >
              Ждать подтверждения целостности сервером
            </Checkbox>
          </LabeledWithTip>
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
            disabled={disabled}
            tip={hint('Базовый адрес экспертной системы ex_system, куда выгружаются записи и куда входит оператор. Смена адреса — опасное изменение (потребует подтверждения): выгрузка пойдёт в другую систему.')}
            onChange={(e) =>
              update((d) => {
                d.sync.server_base_url = e.target.value.trim() === '' ? null : e.target.value;
              })
            }
          />
          <NumField
            label="Размер чанка, МБ"
            value={settings.sync.chunk_size_mb}
            error={errors['chunk_size_mb']}
            disabled={disabled}
            tip={hint('Размер одной части при возобновляемой выгрузке. Меньше — устойчивее к обрывам сети (меньше пере-отправлять), но больше запросов.')}
            onChange={(v) => update((d) => { d.sync.chunk_size_mb = v; })}
          />
          <NumField
            label="Параллельных выгрузок"
            value={settings.sync.parallel_uploads}
            disabled={disabled}
            tip={hint('Сколько записей выгружать одновременно. Больше — быстрее при хорошем канале, но выше нагрузка на сеть и станцию.')}
            onChange={(v) => update((d) => { d.sync.parallel_uploads = v; })}
          />
          <NumField
            label="Бэкофф база, мс"
            value={settings.sync.retry.backoff_base_ms}
            disabled={disabled}
            tip={hint('Начальная пауза перед повторной попыткой выгрузки после ошибки сети. Дальше растёт экспоненциально до «потолка».')}
            onChange={(v) => update((d) => { d.sync.retry.backoff_base_ms = v; })}
          />
          <NumField
            label="Бэкофф потолок, мс"
            value={settings.sync.retry.backoff_max_ms}
            disabled={disabled}
            tip={hint('Максимальная пауза между повторными попытками — дальше интервал не растёт.')}
            onChange={(v) => update((d) => { d.sync.retry.backoff_max_ms = v; })}
          />
          <NumField
            label="Лимит попыток (0 = ∞)"
            value={settings.sync.retry.max_attempts}
            disabled={disabled}
            tip={hint('Сколько раз повторять выгрузку при ошибках. 0 — без лимита, пытаться до успеха (рекомендуется: запись не должна потеряться).')}
            onChange={(v) => update((d) => { d.sync.retry.max_attempts = v; })}
          />
        </Grid>
        <Grid>
          <LabeledWithTip
            label="Авто-выгрузка"
            tip="Выгружать готовую запись на сервер автоматически в фоне, без ручного запуска."
          >
            <Checkbox
              checked={settings.sync.auto_upload}
              disabled={disabled}
              onChange={(e) => update((d) => { d.sync.auto_upload = e.target.checked; })}
            >
              По готовности записи (фон)
            </Checkbox>
          </LabeledWithTip>
          <LabeledWithTip
            label="Откладывать при записи"
            tip="Не выгружать, пока идёт активная запись — чтобы не отнимать ресурсы станции у захвата звука. Выгрузка стартует после остановки сессии."
          >
            <Checkbox
              checked={settings.sync.defer_during_recording}
              disabled={disabled}
              onChange={(e) =>
                update((d) => { d.sync.defer_during_recording = e.target.checked; })
              }
            >
              До конца активной сессии
            </Checkbox>
          </LabeledWithTip>
          <LabeledWithTip
            label="Оффлайн-очередь"
            tip="Если сети нет — копить готовые выгрузки в очередь и отправить автоматически при восстановлении связи."
          >
            <Checkbox
              checked={settings.sync.offline_queue.enabled}
              disabled={disabled}
              onChange={(e) =>
                update((d) => { d.sync.offline_queue.enabled = e.target.checked; })
              }
            >
              Копить выгрузки оффлайн
            </Checkbox>
          </LabeledWithTip>
        </Grid>
      </Card>

      <Card>
        <BlockHead numeral="E" title="Целостность" />
        <Grid>
          <LabeledWithTip
            label="Хеш-цепочка"
            tip="SHA-256 по каждому сегменту, связанные в цепочку: любое изменение записи задним числом становится заметным (защита от подмены). Тот же хеш сервер проверяет при приёме."
          >
            <Checkbox
              checked={settings.integrity.hash_chain}
              disabled={disabled}
              onChange={(e) => update((d) => { d.integrity.hash_chain = e.target.checked; })}
            >
              SHA-256 по сегментам, цепочкой
            </Checkbox>
          </LabeledWithTip>
          <LabeledWithTip
            label="Журнал событий"
            tip="Запись значимых событий с меткой времени и авторством: старт/пауза/обрыв устройства/восстановление, а также изменения настроек. Основа для разбора инцидентов и аудита."
          >
            <Checkbox
              checked={settings.integrity.event_log}
              disabled={disabled}
              onChange={(e) => update((d) => { d.integrity.event_log = e.target.checked; })}
            >
              Старт/пауза/обрыв/изменение настроек
            </Checkbox>
          </LabeledWithTip>
          <LabeledWithTip
            label="ГОСТ ЭЦП"
            tip="Юридически значимая электронная подпись по ГОСТ (КриптоПро). Планируется в фазе 2 — сейчас недоступно."
          >
            <Checkbox checked={settings.integrity.gost_sign} disabled>
              Планируется в фазе 2
            </Checkbox>
          </LabeledWithTip>
        </Grid>
      </Card>

      <Card>
        <BlockHead numeral="F" title="Идентичность и авторизация" />
        <Grid>
          <NumField
            label="Кэш-сессия оператора, ч"
            value={settings.auth.operator.cached_session_hours}
            disabled={disabled}
            tip={hint('Сколько времени действует кэшированная сессия оператора для старта записи в оффлайне (без связи с сервером). По истечении — потребуется онлайн-вход.')}
            onChange={(v) => update((d) => { d.auth.operator.cached_session_hours = v; })}
          />
        </Grid>
        <Grid>
          <LabeledWithTip
            label="Учётка станции"
            tip="У зала своя учётная запись — под ней записи выгружаются в ex_system (транспорт), отдельно от входа оператора."
          >
            <Checkbox
              checked={settings.auth.station_identity.required}
              disabled={disabled}
              onChange={(e) =>
                update((d) => { d.auth.station_identity.required = e.target.checked; })
              }
            >
              Требовать учётку зала
            </Checkbox>
          </LabeledWithTip>
          <LabeledWithTip
            label="Вход оператора"
            tip="Требовать вход оператора перед стартом записи (фиксирует, кто вёл заседание). Уже идущую запись истечение токена или выход не прерывает."
          >
            <Checkbox
              checked={settings.auth.operator.required_to_start}
              disabled={disabled}
              onChange={(e) =>
                update((d) => { d.auth.operator.required_to_start = e.target.checked; })
              }
            >
              Обязателен перед стартом
            </Checkbox>
          </LabeledWithTip>
          <LabeledWithTip
            label="Запись переживает токен"
            tip="Идущая запись не прерывается, если у оператора истёк токен доступа или он вышел. Надёжность важнее строгости сессии — заседание не должно оборваться."
          >
            <Checkbox
              checked={settings.auth.recording_survives_token_expiry}
              disabled={disabled}
              onChange={(e) =>
                update((d) => { d.auth.recording_survives_token_expiry = e.target.checked; })
              }
            >
              Не прерывать при истечении
            </Checkbox>
          </LabeledWithTip>
        </Grid>
      </Card>

      <Card>
        <BlockHead numeral="G" title="Кэш дел" />
        <Grid>
          <Field
            label="Область кэша"
            value={settings.case_cache.scope}
            disabled={disabled}
            tip={hint('Какие дела кэшировать для оффлайн-привязки записи к делу: например, докет текущего суда/зала, доступный станции.')}
            onChange={(e) => update((d) => { d.case_cache.scope = e.target.value; })}
          />
          <NumField
            label="Свежесть кэша, ч"
            value={settings.case_cache.ttl_hours}
            disabled={disabled}
            tip={hint('Через сколько часов кэш дел считается устаревшим — станция показывает индикатор «данные могли измениться» и предлагает обновить.')}
            onChange={(v) => update((d) => { d.case_cache.ttl_hours = v; })}
          />
          <NumField
            label="Лимит записей"
            value={settings.case_cache.max_records}
            disabled={disabled}
            tip={hint('Максимум дел в локальном кэше. Ограничение — минимизация персональных данных на станции.')}
            onChange={(v) => update((d) => { d.case_cache.max_records = v; })}
          />
        </Grid>
        <Grid>
          <LabeledWithTip
            label="Кэшировать дела"
            tip="Держать локальный список дел, чтобы привязывать запись к делу без связи с сервером (в оффлайн-зале)."
          >
            <Checkbox
              checked={settings.case_cache.enabled}
              disabled={disabled}
              onChange={(e) => update((d) => { d.case_cache.enabled = e.target.checked; })}
            >
              Для оффлайн-привязки
            </Checkbox>
          </LabeledWithTip>
          <LabeledWithTip
            label="Шифровать кэш"
            tip="Кэш дел содержит персональные данные (ФИО, номера дел) — шифровать его на диске тем же ключом станции. Отключать не рекомендуется."
          >
            <Checkbox
              checked={settings.case_cache.encrypt}
              disabled={disabled}
              onChange={(e) => update((d) => { d.case_cache.encrypt = e.target.checked; })}
            >
              Содержит ПДн (ФИО/№)
            </Checkbox>
          </LabeledWithTip>
        </Grid>
      </Card>

      <Card>
        <BlockHead
          numeral="H"
          title="Разграничение доступа"
          hint="Кто что может менять на станции. Инфраструктуру и безопасность (сервер, шифрование, хранение, надёжность) правит только администратор — после разблокировки по PIN; оператор меняет лишь свою секцию на экране «Настройки». Сам PIN хранится как хеш и задаётся при развёртывании станции, в настройках его нет."
        />
        <Grid>
          <LabeledWithTip
            label="Требовать админ-PIN"
            tip="Включено: изменить настройки этого раздела можно только после ввода админ-PIN, и проверка идёт в ядре — обойти её через интерфейс нельзя. Выключено: раздел открыт, админ-настройки сможет менять любой оператор без PIN (подходит для станции с одним ответственным)."
          >
            <Checkbox
              checked={settings.admin.pin.required}
              disabled={disabled}
              onChange={(e) => update((d) => { d.admin.pin.required = e.target.checked; })}
            >
              Спрашивать PIN перед изменением админ-настроек
            </Checkbox>
          </LabeledWithTip>
          <NumField
            label="Минимальная длина PIN"
            value={settings.admin.pin.min_length}
            error={errors['admin_pin_min']}
            disabled={disabled}
            tip={hint('Сколько символов минимум должен содержать админ-PIN. Проверяется при установке PIN во время развёртывания станции; на уже заданный PIN не влияет.')}
            onChange={(v) => update((d) => { d.admin.pin.min_length = v; })}
          />
        </Grid>
      </Card>
    </>
  );
}

// ── Многоканальная запись по ролям (этап 09) ─────────────────────────────────

/**
 * Секция «Многоканальная запись»: тумблер режима и карта дорожек «устройство/
 * канал → роль». Админ-скоуп (карта дорожек — инфраструктура зала). Справочник
 * ролей — оператор-скоуп (редактируется на экране «Настройки»).
 */
function MultichannelCard({
  settings,
  errors,
  devices,
  update,
  disabled,
}: {
  settings: Settings;
  errors: Record<string, string>;
  devices: DeviceInfo[];
  update: UpdateFn;
  disabled: boolean;
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
        hint="Фаза 2: N синхронных дорожек, привязанных к ролям (судья, защита, …). Справочник ролей — на экране «Настройки» (оператор)."
      />
      <Grid>
        <LabeledWithTip
          label="Режим"
          tip="Включает запись нескольких дорожек одновременно, каждая привязана к роли говорящего. Выключено — обычная запись одним каналом (как в разделе «Аудио»)."
        >
          <Checkbox
            checked={enabled}
            disabled={disabled}
            onChange={(e) => update((d) => { d.audio.multichannel.enabled = e.target.checked; })}
          >
            Включить многоканальный захват
          </Checkbox>
        </LabeledWithTip>
        <LabeledWithTip
          label="Сведённый мастер"
          tip="Дополнительно к пофайловым дорожкам создаёт один общий файл-микс всех дорожек — удобно для быстрого прослушивания всего заседания целиком."
        >
          <Checkbox
            checked={audio.master_downmix.enabled}
            disabled={disabled || !enabled}
            onChange={(e) => update((d) => { d.audio.master_downmix.enabled = e.target.checked; })}
          >
            Микс всех дорожек в один файл
          </Checkbox>
        </LabeledWithTip>
      </Grid>

      {enabled && (
        <>
          <Grid>
            <LabeledWithTip
              label="Опорная дорожка синхронизации"
              tip="Эталон времени: по частоте дискретизации этой дорожки выравниваются остальные при раздельных устройствах (компенсация дрейфа). На одном многоканальном интерфейсе с общим клоком дрейфа почти нет — параметр не срабатывает."
            >
              <Select
                ariaLabel="Опорная дорожка синхронизации"
                value={String(audio.sync.clock_master_track)}
                disabled={disabled}
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
              disabled={disabled}
              tip={hint('Допустимое расхождение времени между дорожками (при записи с разных устройств их часы немного расходятся). Превышение фиксируется в журнале; при включённой компенсации — выравнивается.')}
              onChange={(v) => update((d) => { d.audio.sync.drift_threshold_ms = v; })}
            />
            <LabeledWithTip
              label="Компенсация дрейфа"
              tip="Автоматически выравнивает дорожки, записанные с разных устройств (добавляя/убирая отсчёты), чтобы они не «расходились» по времени. На одном многоканальном интерфейсе с общими часами не нужна."
            >
              <Checkbox
                checked={audio.sync.drift_compensate}
                disabled={disabled}
                onChange={(e) => update((d) => { d.audio.sync.drift_compensate = e.target.checked; })}
              >
                Выравнивать раздельные устройства
              </Checkbox>
            </LabeledWithTip>
          </Grid>

          <div style={{ marginTop: 16 }}>
            <TracksEditor
              tracks={audio.tracks}
              roles={audio.roles}
              devices={devices}
              error={errors['tracks']}
              disabled={disabled}
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

// Нейтральная кнопка на светлой карточке: вариант `secondary` дизайн-системы
// рассчитан на тёмную панель (светлый текст `--on-dark`), поэтому на бумаге его
// нужно переопределить под тёмный текст/рамку — иначе кнопка «исчезает».
export const NEUTRAL_BTN: CSSProperties = { color: 'var(--ink)', borderColor: 'var(--ink-soft)' };

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
  disabled,
  onAdd,
  onRemove,
  onPatch,
}: {
  tracks: TrackConfig[];
  roles: string[];
  devices: DeviceInfo[];
  error?: string;
  disabled: boolean;
  onAdd: () => void;
  onRemove: (i: number) => void;
  onPatch: (i: number, patch: Partial<TrackConfig>) => void;
}) {
  const deviceOptions = [
    { value: '', label: 'Системное по умолчанию' },
    ...devices.map((d) => ({
      value: d.name,
      label: d.is_default ? `${d.name} · по умолчанию` : d.name,
    })),
  ];

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      <span style={tracksCaptionStyle}>Дорожки · карта «устройство/канал → роль»</span>

      {tracks.length === 0 && <Tag tone="accent">Добавьте хотя бы одну дорожку</Tag>}

      {tracks.map((t, i) => (
        <div
          key={i}
          style={{
            display: 'grid',
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
              disabled={disabled}
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
            disabled={disabled}
            onChange={(e) => {
              const n = Number(e.target.value);
              if (!Number.isNaN(n)) onPatch(i, { channel_index: n });
            }}
          />
          <Labeled label="Роль">
            <Select
              ariaLabel={`Роль дорожки ${i + 1}`}
              value={t.role}
              disabled={disabled}
              onChange={(v) => onPatch(i, { role: v })}
              options={roles.map((r) => ({ value: r, label: r }))}
              triggerStyle={selectTriggerStyle}
            />
          </Labeled>
          <Field
            label="Метка"
            placeholder="Свидетель у трибуны"
            value={t.label}
            disabled={disabled}
            onChange={(e) => onPatch(i, { label: e.target.value })}
          />
          <RemoveTrackButton index={i} disabled={disabled} onClick={() => onRemove(i)} />
        </div>
      ))}

      {error && <Tag tone="accent">{error}</Tag>}

      {!disabled && (
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
      )}
    </div>
  );
}

function RemoveTrackButton({
  index,
  disabled,
  onClick,
}: {
  index: number;
  disabled: boolean;
  onClick: () => void;
}) {
  const [hover, setHover] = useState(false);
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
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
        color: disabled ? 'var(--muted)' : hover ? 'var(--accent-deep)' : 'var(--accent)',
        cursor: disabled ? 'default' : 'pointer',
        transition: 'color 120ms ease',
      }}
    >
      <Icon name="icon-close" size={16} decorative />
    </button>
  );
}

// ── Раскладка/обёртки ────────────────────────────────────────────────────────

export function SectionTitle({ children }: { children: ReactNode }) {
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

export function Grid({ children }: { children: ReactNode }) {
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

const tracksCaptionStyle: CSSProperties = { ...labeledCaptionStyle };

export function Labeled({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <span style={labeledCaptionStyle}>{label}</span>
      {children}
    </div>
  );
}

export function LabeledWithTip({
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

export function NumField({
  label,
  value,
  error,
  disabled,
  tip,
  onChange,
}: {
  label: string;
  value: number;
  error?: string;
  disabled?: boolean;
  tip?: ReactNode;
  onChange: (v: number) => void;
}) {
  return (
    <Field
      label={label}
      type="number"
      value={String(value)}
      error={error}
      disabled={disabled}
      tip={tip}
      onChange={(e) => {
        const n = Number(e.target.value);
        if (!Number.isNaN(n)) onChange(n);
      }}
    />
  );
}

/** Короткий помощник: подсказка «ⓘ» с единым aria-именем для настроек. */
export function hint(text: string): ReactNode {
  return <InfoTip text={text} label="Что это?" />;
}
