import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { BlockHead, Button, Card, NEUTRAL_BTN, Skeleton, Tag } from '../design';
import {
  runSelfTest,
  type CheckStatus,
  type SelfTestCheck,
  type SelfTestFix,
  type SelfTestReport,
} from '../lib/core';

// Проверка перед заседанием (этап 10.6, deliverable 1). Однокнопочный self-test:
// устройство, диск, сервер, вход оператора, незавершённые сессии. Классификация —
// в ядре (ipc::selftest_cmds); здесь только запуск команды и отображение чек-листа
// с действиями «Исправить». Переиспользуется на экранах «Запись»/«Диагностика» и
// как финальный шаг мастера первого запуска.

type Tone = 'default' | 'accent' | 'gold' | 'green';

/** Тон плашки по статусу проверки (ok → зелёный, warn → золото, fail → акцент). */
function statusTone(status: CheckStatus): Tone {
  switch (status) {
    case 'ok':
      return 'green';
    case 'warn':
      return 'gold';
    case 'fail':
      return 'accent';
  }
}

function statusLabel(status: CheckStatus): string {
  switch (status) {
    case 'ok':
      return 'Готово';
    case 'warn':
      return 'Внимание';
    case 'fail':
      return 'Проблема';
  }
}

/** Куда ведёт кнопка «Исправить» для данного действия ядра. */
const FIX_ROUTE: Record<SelfTestFix, string> = {
  open_settings: '/settings',
  open_record: '/',
  open_login: '/login',
};

const FIX_LABEL: Record<SelfTestFix, string> = {
  open_settings: 'К настройкам',
  open_record: 'К записи',
  open_login: 'Ко входу',
};

type Load =
  | { kind: 'loading' }
  | { kind: 'ready'; report: SelfTestReport }
  | { kind: 'error'; message: string };

export function SelfTestPanel({
  numeral = '✓',
  onReady,
}: {
  /** Нумерал блока (для встраивания в разные экраны/мастер). */
  numeral?: string;
  /** Колбэк с агрегатом «можно начинать» после каждого прогона (для мастера). */
  onReady?: (ready: boolean) => void;
}) {
  const navigate = useNavigate();
  const [load, setLoad] = useState<Load>({ kind: 'loading' });

  const run = useCallback(() => {
    setLoad({ kind: 'loading' });
    let active = true;
    runSelfTest()
      .then((report) => {
        if (!active) return;
        setLoad({ kind: 'ready', report });
        onReady?.(report.ready);
      })
      .catch((e: unknown) => active && setLoad({ kind: 'error', message: describeError(e) }));
    return () => {
      active = false;
    };
  }, [onReady]);

  useEffect(() => run(), [run]);

  return (
    <Card>
      <div style={{ display: 'flex', alignItems: 'flex-start', gap: 12, flexWrap: 'wrap' }}>
        <div style={{ flex: 1, minWidth: 220 }}>
          <BlockHead
            numeral={numeral}
            title="Проверка перед заседанием"
            hint="Устройство, место на диске, сервер, вход оператора и незавершённые сессии"
          />
        </div>
        <Button
          variant="secondary"
          style={NEUTRAL_BTN}
          loading={load.kind === 'loading'}
          onClick={run}
        >
          Проверить снова
        </Button>
      </div>

      {load.kind === 'loading' && (
        <div style={{ marginTop: 12 }}>
          <Skeleton variant="block" count={5} height={40} />
        </div>
      )}

      {load.kind === 'error' && (
        <div style={{ marginTop: 12 }}>
          <Tag tone="accent">Ошибка проверки: {load.message}</Tag>
        </div>
      )}

      {load.kind === 'ready' && (
        <>
          <div
            role="status"
            aria-live="polite"
            style={{ display: 'flex', alignItems: 'center', gap: 10, margin: '12px 0' }}
          >
            {load.report.ready ? (
              <Tag tone="green" style={{ fontSize: 14, padding: '4px 12px' }}>
                ✓ Можно начинать
              </Tag>
            ) : (
              <Tag tone="accent" style={{ fontSize: 14, padding: '4px 12px' }}>
                Есть проблемы — исправьте отмеченное
              </Tag>
            )}
          </div>

          <ul style={{ listStyle: 'none', margin: 0, padding: 0, display: 'flex', flexDirection: 'column', gap: 10 }}>
            {load.report.checks.map((c) => (
              <CheckRow key={c.id} check={c} onFix={(to) => navigate(to)} />
            ))}
          </ul>
        </>
      )}
    </Card>
  );
}

function CheckRow({
  check,
  onFix,
}: {
  check: SelfTestCheck;
  onFix: (to: string) => void;
}) {
  const fix = check.fix ?? null;
  return (
    <li
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 12,
        flexWrap: 'wrap',
        paddingBottom: 10,
        borderBottom: '1px solid var(--hairline)',
      }}
    >
      <Tag tone={statusTone(check.status)} aria-label={statusLabel(check.status)}>
        {statusLabel(check.status)}
      </Tag>
      <div style={{ flex: 1, minWidth: 200 }}>
        <div style={{ fontSize: 14, fontWeight: 500, color: 'var(--ink)' }}>{check.label}</div>
        <div style={{ fontSize: 12, color: 'var(--ink-soft)', marginTop: 2 }}>{check.detail}</div>
      </div>
      {check.status !== 'ok' && fix && (
        <Button variant="mini" onClick={() => onFix(FIX_ROUTE[fix])}>
          {FIX_LABEL[fix]}
        </Button>
      )}
    </li>
  );
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return 'неизвестная ошибка';
}
