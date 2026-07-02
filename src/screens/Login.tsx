import { useCallback, useState, type ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import { BlockHead, Button, Card, CriticalNotice, Field, Tag } from '../design';
import { authLogin, authUnlockOffline } from '../lib/core';
import { useAuth } from '../lib/auth-context';

// Экран входа оператора (этап 10.3). Онлайн-вход по логину/паролю (JWT ex_system)
// + PIN, который затем разблокирует оффлайн-старт. Когда доступна валидная
// кэшированная сессия без связи — предлагает оффлайн-разблокировку по PIN.
// Управление учётками/ролями/паролями — в ex_system; станция лишь входит.

export function LoginScreen() {
  const navigate = useNavigate();
  const { status, refresh } = useAuth();

  const pinRequired = status?.pin_required ?? true;
  const offlineAvailable = status ? status.offline_cached && !status.operator : false;

  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [pin, setPin] = useState('');
  const [offlinePin, setOfflinePin] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // При наличии кэша станция по умолчанию предлагает оффлайн-вход; онлайн-форму
  // показываем лишь по явному запросу (напр. вход другим оператором). На экране —
  // всегда одна форма; переключение между ними по ссылке (когда есть кэш).
  const [showOnlineForm, setShowOnlineForm] = useState(false);

  const showOffline = offlineAvailable && !showOnlineForm;
  const showOnline = !showOffline;

  const goHome = useCallback(() => {
    refresh();
    navigate('/', { replace: true });
  }, [navigate, refresh]);

  const onLogin = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      await authLogin(email, password, pinRequired ? pin : undefined);
      goHome();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }, [email, password, pin, pinRequired, goHome]);

  const onUnlock = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      await authUnlockOffline(pinRequired ? offlinePin : undefined);
      goHome();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }, [offlinePin, pinRequired, goHome]);

  return (
    <div
      style={{
        minHeight: '100vh',
        display: 'grid',
        placeItems: 'center',
        background: 'var(--paper)',
        padding: 24,
      }}
    >
      <div style={{ display: 'flex', flexDirection: 'column', gap: 16, width: '100%', maxWidth: 440 }}>
        <div style={{ textAlign: 'center', marginBottom: 4 }}>
          <div style={{ fontFamily: 'var(--serif)', fontSize: 24, fontWeight: 500 }}>
            Аудиопротокол
          </div>
          <div style={{ color: 'var(--muted)', fontSize: 13 }}>Вход оператора</div>
        </div>

        {error && (
          <CriticalNotice variant="critical" title="Не удалось войти" description={error} />
        )}

        {showOffline && (
          <Card>
            <BlockHead
              numeral=""
              title="Оффлайн-режим"
              hint="Нет связи с сервером — вход по кэшированной сессии"
            />
            <div style={{ marginTop: 8, marginBottom: 12 }}>
              <Tag tone="accent">Доступна кэшированная сессия оператора</Tag>
            </div>
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void onUnlock();
              }}
              style={{ display: 'flex', flexDirection: 'column', gap: 14 }}
            >
              {pinRequired && (
                <Field
                  label="PIN"
                  name="offlinePin"
                  type="password"
                  inputMode="numeric"
                  value={offlinePin}
                  onChange={(e) => setOfflinePin(e.target.value)}
                />
              )}
              <Button variant="primary" type="submit" disabled={busy}>
                {busy ? 'Вход…' : 'Войти'}
              </Button>
            </form>
            <p
              style={{
                marginTop: 14,
                textAlign: 'center',
                fontSize: 13,
                color: 'var(--muted)',
              }}
            >
              Другой оператор?{' '}
              <TextLink onClick={() => setShowOnlineForm(true)}>
                Войти по учётной записи
              </TextLink>
            </p>
          </Card>
        )}

        {showOnline && (
          <Card>
            <BlockHead
              numeral=""
              title="Вход по учётной записи"
              hint="Логин и пароль оператора в системе ex_system"
            />
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void onLogin();
              }}
              style={{ display: 'flex', flexDirection: 'column', gap: 14, marginTop: 12 }}
            >
              <Field
                label="Логин (email)"
                name="email"
                type="email"
                autoComplete="username"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="operator@court"
              />
              <Field
                label="Пароль"
                name="password"
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
              {pinRequired && (
                <Field
                  label="PIN для оффлайн-старта"
                  name="pin"
                  type="password"
                  inputMode="numeric"
                  value={pin}
                  onChange={(e) => setPin(e.target.value)}
                  placeholder="Задайте PIN для работы без связи"
                />
              )}
              <Button
                variant="primary"
                type="submit"
                disabled={busy || !email || !password}
              >
                {busy ? 'Вход…' : 'Войти'}
              </Button>
            </form>
            {offlineAvailable && (
              <p
                style={{
                  marginTop: 14,
                  textAlign: 'center',
                  fontSize: 13,
                  color: 'var(--muted)',
                }}
              >
                Нет связи с сервером?{' '}
                <TextLink onClick={() => setShowOnlineForm(false)}>
                  Оффлайн-вход по PIN
                </TextLink>
              </p>
            )}
          </Card>
        )}
      </div>
    </div>
  );
}

// Инлайн-ссылка в стиле дизайн-системы (акцентный текст с подчёркиванием,
// hover → --accent-deep). Семантически кнопка (действие, не навигация), но
// визуально — активная ссылка.
function TextLink({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  const [hover, setHover] = useState(false);
  return (
    <button
      type="button"
      onClick={onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        background: 'transparent',
        border: 'none',
        padding: 0,
        font: 'inherit',
        fontWeight: 500,
        color: hover ? 'var(--accent-deep)' : 'var(--accent)',
        textDecoration: 'underline',
        textUnderlineOffset: 3,
        cursor: 'pointer',
        transition: 'color 120ms ease',
      }}
    >
      {children}
    </button>
  );
}

function describeError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
