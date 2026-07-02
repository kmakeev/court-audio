import { useCallback, useEffect, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { BlockHead, Button, Card, CriticalNotice, Field, ProgressBar, Select, Tag, type SelectOption } from '../design';
import { ConfirmDialog } from '../shell/ConfirmDialog';
import {
  exportBuildPackage,
  exportBurnDvd,
  exportDvdDriveStatus,
  exportSessionInfo,
  formatAdjudicationRef,
  onExportProgress,
  type DvdBurnResultView,
  type DvdDriveView,
  type ExportComposition,
  type ExportFormat,
  type ExportResult,
  type ExportSessionInfo,
} from '../lib/core';
import { getSettings, type Settings } from '../lib/settings';

// Экран «Экспорт» (этап 10.2). Открывается кнопкой «Экспортировать» из
// списка сессий. Мастер: состав/формат/назначение → сборка пакета (прогресс)
// → сводка (файлы, пути, опц. прожиг DVD). Дешифровка/склейка/FLAC/HTML-плеер
// — в ядре (export_cmds); здесь только выбор параметров и отображение.

const NEUTRAL_BTN = { color: 'var(--ink)', borderColor: 'var(--ink-soft)' } as const;

const FORMAT_OPTIONS: SelectOption[] = [
  { value: 'wav_pcm', label: 'WAV (без потерь)' },
  { value: 'flac', label: 'FLAC (компактнее, без потерь)' },
];

const DESTINATION_OPTIONS: SelectOption[] = [
  { value: 'folder', label: 'Папка (в т.ч. USB-носитель)' },
  { value: 'dvd', label: 'DVD' },
];

type Load =
  | { kind: 'loading' }
  | { kind: 'ready'; info: ExportSessionInfo }
  | { kind: 'error'; message: string };

type Step = 'form' | 'progress' | 'summary';
type DestinationKind = 'folder' | 'dvd';

function compositionOptions(info: ExportSessionInfo): SelectOption[] {
  if (info.tracks.length <= 1) {
    // Одна дорожка — либо явно настроенная (многоканал, этап 09), либо
    // легаси-заглушка `SINGLE_TRACK_ROLE = "single"` для одноканальной
    // v1-сессии (`store::manifest::ManifestStore::resolve_tracks`, Rust).
    // Этот внутренний код НЕ входит в настраиваемый справочник `audio.roles`
    // и никогда не должен попадать в интерфейс — здесь только пользовательская
    // метка либо нейтральное «Запись» (в отличие от многодорожечного списка
    // ниже, где `t.role` — настоящий код из `audio.roles`, показывается как есть).
    const only = info.tracks[0];
    return [{ value: 'all_tracks', label: only?.label.trim() || 'Запись' }];
  }
  return [
    { value: 'all_tracks', label: 'Все дорожки' },
    { value: 'mix', label: 'Сведённый микс' },
    ...info.tracks.map((t) => ({
      value: `track:${t.track_id}`,
      label: t.label.trim() || t.role,
    })),
  ];
}

function compositionValue(c: ExportComposition): string {
  if (c.kind === 'mix') return 'mix';
  if (c.kind === 'track') return `track:${c.track_id}`;
  return 'all_tracks';
}

function parseComposition(value: string): ExportComposition {
  if (value === 'mix') return { kind: 'mix' };
  if (value.startsWith('track:')) {
    return { kind: 'track', track_id: Number(value.slice('track:'.length)) };
  }
  return { kind: 'all_tracks' };
}

export function ExportScreen() {
  const params = useParams();
  const navigate = useNavigate();
  const dir = params.dir ? decodeURIComponent(params.dir) : '';

  const [settings, setSettings] = useState<Settings | null>(null);
  const [load, setLoad] = useState<Load>({ kind: 'loading' });
  const [step, setStep] = useState<Step>('form');

  const [composition, setComposition] = useState<ExportComposition>({ kind: 'all_tracks' });
  const [format, setFormat] = useState<ExportFormat>('wav_pcm');
  const [destinationKind, setDestinationKind] = useState<DestinationKind>('folder');
  const [destinationPath, setDestinationPath] = useState('');

  const [dvdDrive, setDvdDrive] = useState<DvdDriveView | null>(null);
  const [dvdChecked, setDvdChecked] = useState(false);

  const [confirmOpen, setConfirmOpen] = useState(false);
  const [progress, setProgress] = useState<{ stage: string; percent: number } | null>(null);
  const [result, setResult] = useState<ExportResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const [burning, setBurning] = useState(false);
  const [burnResult, setBurnResult] = useState<DvdBurnResultView | null>(null);
  const [burnError, setBurnError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    Promise.all([getSettings(), exportSessionInfo(dir)])
      .then(([s, info]) => {
        if (!active) return;
        setSettings(s);
        setLoad({ kind: 'ready', info });
        setFormat(s.export.default_codec);
        setComposition(
          info.tracks.length > 1
            ? { kind: 'all_tracks' }
            : { kind: 'track', track_id: info.tracks[0]?.track_id ?? 0 },
        );
      })
      .catch((e: unknown) => active && setLoad({ kind: 'error', message: describeError(e) }));
    return () => {
      active = false;
    };
  }, [dir]);

  // Проверка привода — только при выборе назначения «DVD» (не на каждый рендер).
  useEffect(() => {
    if (destinationKind !== 'dvd' || dvdChecked) return;
    let active = true;
    exportDvdDriveStatus()
      .then((d) => active && setDvdDrive(d))
      .catch(() => active && setDvdDrive(null))
      .finally(() => active && setDvdChecked(true));
    return () => {
      active = false;
    };
  }, [destinationKind, dvdChecked]);

  // Прогресс сборки — только пока показан шаг «Сборка».
  useEffect(() => {
    if (step !== 'progress') return;
    let active = true;
    const unlisteners: Array<() => void> = [];
    const wire = async () => {
      const un = await onExportProgress((e) => {
        if (active) setProgress({ stage: e.stage, percent: e.percent });
      });
      if (active) unlisteners.push(un);
      else un();
    };
    void wire();
    return () => {
      active = false;
      unlisteners.forEach((u) => u());
    };
  }, [step]);

  const runBuild = useCallback(
    (confirmed: boolean) => {
      setError(null);
      setStep('progress');
      setProgress({ stage: 'joining', percent: 0 });
      exportBuildPackage(
        dir,
        composition,
        format,
        destinationKind === 'folder' ? destinationPath.trim() || null : null,
        confirmed,
      )
        .then((r) => {
          setResult(r);
          setStep('summary');
        })
        .catch((e: unknown) => {
          setError(describeError(e));
          setStep('form');
        });
    },
    [dir, composition, format, destinationKind, destinationPath],
  );

  const onStart = useCallback(() => {
    if (settings?.export.policy === 'requires_confirmation') {
      setConfirmOpen(true);
      return;
    }
    runBuild(false);
  }, [settings, runBuild]);

  const onBurnDvd = useCallback(() => {
    if (!result || !dvdDrive) return;
    setBurning(true);
    setBurnError(null);
    exportBurnDvd(dir, result.package_dir, dvdDrive.id)
      .then((r) => setBurnResult(r))
      .catch((e: unknown) => setBurnError(describeError(e)))
      .finally(() => setBurning(false));
  }, [dir, result, dvdDrive]);

  const backButton = (
    <Button variant="secondary" style={NEUTRAL_BTN} onClick={() => navigate('/sessions')}>
      ← К сессиям
    </Button>
  );

  if (load.kind === 'loading') {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
        <div>{backButton}</div>
        <Card>
          <BlockHead numeral="↓" title="Экспорт записи" hint="Загрузка сессии…" />
        </Card>
      </div>
    );
  }

  if (load.kind === 'error') {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
        <div>{backButton}</div>
        <Card>
          <BlockHead numeral="↓" title="Экспорт записи" />
          <Tag tone="accent">Ошибка: {load.message}</Tag>
        </Card>
      </div>
    );
  }

  const { info } = load;
  const policy = settings?.export.policy ?? 'allowed';

  if (policy === 'forbidden') {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
        <div>{backButton}</div>
        <Card>
          <BlockHead
            numeral="↓"
            title="Экспорт записи"
            hint={formatAdjudicationRef(info.adjudication_ref) ?? 'Без привязки к делу'}
          />
          <CriticalNotice
            title="Экспорт запрещён администратором станции"
            description="Обратитесь к администратору станции, чтобы изменить настройку export.policy."
          />
        </Card>
      </div>
    );
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 20, maxWidth: 880 }}>
      <div>{backButton}</div>

      <Card>
        <BlockHead
          numeral="↓"
          title="Экспорт записи"
          hint={formatAdjudicationRef(info.adjudication_ref) ?? 'Без привязки к делу'}
        />
        <Tag tone={info.integrity_ok ? 'green' : 'accent'}>
          {info.integrity_ok ? 'Целостность подтверждена' : 'Целостность не подтверждена'}
        </Tag>
      </Card>

      {error && <Tag tone="accent">Ошибка: {error}</Tag>}

      {step === 'form' && (
        <>
          <Card>
            <BlockHead numeral="A" title="Состав" />
            <Select
              ariaLabel="Состав пакета"
              value={compositionValue(composition)}
              onChange={(v) => setComposition(parseComposition(v))}
              options={compositionOptions(info)}
            />
          </Card>

          <Card>
            <BlockHead numeral="B" title="Формат" />
            <Select
              ariaLabel="Формат аудио"
              value={format}
              onChange={(v) => setFormat(v as ExportFormat)}
              options={FORMAT_OPTIONS}
            />
          </Card>

          <Card>
            <BlockHead numeral="C" title="Назначение" />
            <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
              <Select
                ariaLabel="Назначение"
                value={destinationKind}
                onChange={(v) => {
                  setDestinationKind(v as DestinationKind);
                  setDvdChecked(false);
                }}
                options={DESTINATION_OPTIONS}
              />
              {destinationKind === 'folder' && (
                <Field
                  label="Путь к папке (пусто — папка станции по умолчанию)"
                  value={destinationPath}
                  onChange={(e) => setDestinationPath(e.target.value)}
                  placeholder="/Volumes/USB/протокол"
                />
              )}
              {destinationKind === 'dvd' && dvdChecked && !dvdDrive && (
                <CriticalNotice
                  variant="warning"
                  title="Привод/утилита прожига не найдены"
                  description="Пакет можно собрать и выдать через папку — назначение можно сменить в любой момент."
                />
              )}
              {destinationKind === 'dvd' && dvdDrive && (
                <Tag tone="green">Найден привод: {dvdDrive.label}</Tag>
              )}
            </div>
          </Card>

          <Card>
            <Button variant="primary" onClick={onStart}>
              Начать экспорт
            </Button>
          </Card>
        </>
      )}

      {step === 'progress' && (
        <Card>
          <BlockHead numeral="D" title="Сборка пакета" />
          <ProgressBar label={`Экспорт… ${progress?.stage ?? ''}`} value={progress?.percent} />
        </Card>
      )}

      {step === 'summary' && result && (
        <Card>
          <BlockHead numeral="E" title="Готово" hint={result.package_dir} />
          <ul
            style={{
              listStyle: 'none',
              margin: '0 0 16px',
              padding: 0,
              display: 'flex',
              flexDirection: 'column',
              gap: 8,
            }}
          >
            {result.files.map((f) => (
              <li key={f.name} style={{ fontSize: 12, color: 'var(--ink-soft)' }}>
                <span className="num">{f.name}</span> — {(f.size_bytes / 1024).toFixed(0)} КБ ·{' '}
                <span className="num">{f.sha256.slice(0, 12)}…</span>
              </li>
            ))}
          </ul>
          <p style={{ fontSize: 12, color: 'var(--muted)', marginBottom: 16 }}>
            Манифест: <span className="num">{result.manifest_path}</span>
            <br />
            Автономный плеер: <span className="num">{result.player_path}</span>
          </p>

          {destinationKind === 'dvd' && dvdDrive && !burnResult && (
            <Button variant="primary" loading={burning} onClick={onBurnDvd}>
              Записать на DVD ({dvdDrive.label})
            </Button>
          )}
          {burnError && (
            <div style={{ marginTop: 12 }}>
              <Tag tone="accent">Ошибка прожига: {burnError}</Tag>
            </div>
          )}
          {burnResult && (
            <div style={{ marginTop: 12 }}>
              <Tag tone={burnResult.verified ? 'green' : 'accent'}>
                {burnResult.verified ? 'DVD прожжён и верифицирован' : 'Верификация DVD не прошла'}
              </Tag>
            </div>
          )}
        </Card>
      )}

      <ConfirmDialog
        open={confirmOpen}
        title="Подтвердите экспорт"
        description="Экспорт выводит запись из зашифрованного хранилища станции. Действие будет зафиксировано в журнале."
        confirmLabel="Экспортировать"
        tone="neutral"
        onConfirm={() => {
          setConfirmOpen(false);
          runBuild(true);
        }}
        onCancel={() => setConfirmOpen(false)}
      />

      <div>{backButton}</div>
    </div>
  );
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}
