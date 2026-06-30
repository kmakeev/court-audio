import { useCallback, useEffect, useRef, useState } from 'react';
import { Button, Field, Tag } from '../design';
import {
  getCaseCacheStatus,
  searchCases,
  syncCaseCache,
  type AdjudicationRef,
  type CaseCacheStatus,
  type CaseRecord,
} from '../lib/core';

// Пикер привязки записи к делу (этап 05). Два режима: автокомплит по локальному
// (оффлайн, зашифрованному) кэшу докета → `resolved`; и ручной ввод № как
// фолбэк → `manual` (pending). Сам пикер — контролируемый редактор привязки:
// он формирует AdjudicationRef и отдаёт наверх; персист в манифест делает экран
// «Запись» (привязка к активной сессии). Параметры кэша (scope/ttl/limit) —
// из реестра настроек, тянутся командами ядра; здесь «магических чисел» нет.

// Сколько результатов автокомплита показывать (косметика выпадающего списка,
// не бизнес-параметр: лимит самого кэша — case_cache.max_records на станции).
const MAX_SUGGESTIONS = 8;

interface CasePickerProps {
  binding: AdjudicationRef | null;
  onChange: (next: AdjudicationRef | null) => void;
}

type Mode = 'cache' | 'manual';

export function CasePicker({ binding, onChange }: CasePickerProps) {
  const [status, setStatus] = useState<CaseCacheStatus | null>(null);
  const [mode, setMode] = useState<Mode>('cache');
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<CaseRecord[]>([]);
  const [open, setOpen] = useState(false);
  const [manualNumber, setManualNumber] = useState('');
  const [manualFio, setManualFio] = useState('');
  const [syncing, setSyncing] = useState(false);
  const [syncNote, setSyncNote] = useState<string | null>(null);
  const boxRef = useRef<HTMLDivElement | null>(null);

  const refreshStatus = useCallback(() => {
    getCaseCacheStatus()
      .then(setStatus)
      .catch(() => setStatus(null));
  }, []);

  useEffect(() => {
    refreshStatus();
  }, [refreshStatus]);

  // При пустом кэше предлагаем сразу ручной ввод (оффлайн без кэша — фолбэк).
  useEffect(() => {
    if (status && status.record_count === 0) setMode('manual');
  }, [status]);

  // Поиск по кэшу при вводе запроса (оффлайн, через ядро).
  useEffect(() => {
    if (mode !== 'cache') return;
    let active = true;
    searchCases(query)
      .then((rows) => {
        if (active) setResults(rows.slice(0, MAX_SUGGESTIONS));
      })
      .catch(() => {
        if (active) setResults([]);
      });
    return () => {
      active = false;
    };
  }, [query, mode]);

  // Клик вне выпадающего списка — закрыть.
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (boxRef.current && !boxRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDoc);
    return () => document.removeEventListener('mousedown', onDoc);
  }, [open]);

  const pickCase = useCallback(
    (rec: CaseRecord) => {
      onChange({
        kind: 'resolved',
        adjudication_id: rec.id,
        raw_number: rec.number,
        raw_fio: rec.fio || undefined,
      });
      setQuery(`${rec.number}${rec.fio ? ` · ${rec.fio}` : ''}`);
      setOpen(false);
    },
    [onChange],
  );

  const applyManual = useCallback(
    (numberRaw: string, fioRaw: string) => {
      const number = numberRaw.trim();
      if (!number) {
        onChange(null);
        return;
      }
      onChange({
        kind: 'manual',
        raw_number: number,
        raw_fio: fioRaw.trim() || undefined,
      });
    },
    [onChange],
  );

  const onSync = useCallback(() => {
    setSyncing(true);
    setSyncNote(null);
    syncCaseCache()
      .then((s) => {
        setStatus(s);
        setSyncNote('Кэш дел обновлён.');
      })
      .catch((e: unknown) => {
        setSyncNote(typeof e === 'string' ? e : 'Не удалось обновить кэш.');
      })
      .finally(() => setSyncing(false));
  }, []);

  const switchToManual = useCallback(() => {
    setMode('manual');
    setOpen(false);
    onChange(null);
    applyManual(manualNumber, manualFio);
  }, [applyManual, manualNumber, manualFio, onChange]);

  const switchToCache = useCallback(() => {
    setMode('cache');
    onChange(null);
    setQuery('');
  }, [onChange]);

  return (
    <div style={{ marginTop: 12, maxWidth: 460 }}>
      {/* Шапка: статус привязки + свежесть кэша + обновление. */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 10,
          flexWrap: 'wrap',
          marginBottom: 12,
        }}
      >
        <BindingStatusTag binding={binding} />
        <CacheFreshness status={status} />
        <Button
          variant="mini"
          onClick={onSync}
          loading={syncing}
          aria-label="Обновить кэш дел"
        >
          Обновить кэш
        </Button>
      </div>

      {syncNote && (
        <p style={{ fontSize: 12, color: 'var(--muted)', margin: '0 0 12px' }}>{syncNote}</p>
      )}

      {/* Переключатель режима. */}
      <div style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
        <Button
          variant={mode === 'cache' ? 'primary' : 'link'}
          onClick={switchToCache}
          disabled={mode === 'cache'}
        >
          Из кэша дел
        </Button>
        <Button
          variant={mode === 'manual' ? 'primary' : 'link'}
          onClick={switchToManual}
          disabled={mode === 'manual'}
        >
          Ввести вручную
        </Button>
      </div>

      {mode === 'cache' ? (
        <div ref={boxRef} style={{ position: 'relative' }}>
          <Field
            label="Поиск дела (№ или ФИО)"
            placeholder={
              status && status.record_count > 0
                ? 'начните вводить № дела или ФИО'
                : 'кэш дел пуст — обновите или введите вручную'
            }
            value={query}
            onFocus={() => setOpen(true)}
            onChange={(e) => {
              setQuery(e.target.value);
              setOpen(true);
              // Свободный ввод поверх выбранного дела сбрасывает привязку до выбора.
              if (binding?.kind === 'resolved') onChange(null);
            }}
            role="combobox"
            aria-expanded={open}
            aria-controls="case-suggestions"
          />
          {open && results.length > 0 && (
            <ul
              id="case-suggestions"
              role="listbox"
              style={{
                listStyle: 'none',
                margin: '4px 0 0',
                padding: 4,
                position: 'absolute',
                zIndex: 20,
                left: 0,
                right: 0,
                background: 'var(--paper-elev)',
                border: '1px solid var(--hairline)',
                maxHeight: 260,
                overflowY: 'auto',
                boxShadow: '0 8px 24px rgba(0,0,0,0.12)',
              }}
            >
              {results.map((rec) => (
                <li key={rec.id} role="option" aria-selected={binding?.adjudication_id === rec.id}>
                  <button
                    type="button"
                    onClick={() => pickCase(rec)}
                    style={{
                      display: 'block',
                      width: '100%',
                      textAlign: 'left',
                      padding: '8px 10px',
                      background: 'transparent',
                      border: 'none',
                      cursor: 'pointer',
                      color: 'var(--ink)',
                    }}
                  >
                    <span className="num" style={{ fontWeight: 600 }}>
                      {rec.number}
                    </span>
                    <span style={{ color: 'var(--muted)' }}>
                      {rec.fio ? ` · ${rec.fio}` : ''} · {rec.date}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : (
        <div style={{ display: 'grid', gap: 12 }}>
          <Field
            label="№ дела"
            placeholder="напр. № 1-123/2026"
            value={manualNumber}
            onChange={(e) => {
              setManualNumber(e.target.value);
              applyManual(e.target.value, manualFio);
            }}
          />
          <Field
            label="ФИО сторон (необязательно)"
            placeholder="напр. Иванов И.И."
            value={manualFio}
            onChange={(e) => {
              setManualFio(e.target.value);
              applyManual(manualNumber, e.target.value);
            }}
          />
        </div>
      )}
    </div>
  );
}

function BindingStatusTag({ binding }: { binding: AdjudicationRef | null }) {
  if (!binding) {
    return (
      <Tag tone="default" role="status">
        Дело не привязано
      </Tag>
    );
  }
  if (binding.kind === 'resolved') {
    return (
      <Tag tone="green" role="status">
        ✓ Дело выбрано
      </Tag>
    );
  }
  return (
    <Tag tone="gold" role="status">
      ⚠ Ручной ввод — связывание на сервере
    </Tag>
  );
}

function CacheFreshness({ status }: { status: CaseCacheStatus | null }) {
  if (!status || status.synced_at_unix_ms == null) {
    return (
      <span style={{ fontSize: 12, color: 'var(--muted)' }}>Кэш дел не загружен</span>
    );
  }
  const ageHours = Math.max(0, Math.floor((Date.now() - status.synced_at_unix_ms) / 3_600_000));
  const when = ageHours < 1 ? 'менее часа назад' : `${ageHours} ч назад`;
  return (
    <span style={{ fontSize: 12, color: status.is_fresh ? 'var(--muted)' : 'var(--accent-deep)' }}>
      {status.is_fresh ? 'Кэш актуален' : 'Кэш устарел'} · {status.record_count} дел · обновлён {when}
    </span>
  );
}
