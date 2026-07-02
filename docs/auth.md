# Аутентификация оператора (этап 10.3)

Модуль «Аудиопротокол» реализует зафиксированную политику
([`configuration.md`](configuration.md), раздел «Аутентификация»): **оператор
обязан войти перед стартом записи**; идущая запись истечение токена/выход **не
прерывает**; в оффлайн-зале старт идёт по **кэшированной** сессии оператора.

Управление учётками, ролями и парольными политиками — в `ex_system`; станция
только аутентифицируется, кэширует сессию и подставляет идентичность в
манифест/журнал/выгрузку. Параметры — из реестра `configuration.md`, без
«магических чисел».

## Поток входа (JWT `ex_system`)

Вход — JWT **cookie-flow** `ex_system` (`CookieTokenObtainPairView`, см.
`criminal/users/jwt_views.py` в `ex_system`):

| Шаг | Запрос | Ответ |
|---|---|---|
| Токены | `POST {server_base_url}/api/token/` `{email, password}` | тело `{access}`; **refresh — в httpOnly-cookie `ex_refresh`** (Path `/api/token/`) |
| Обновление | `POST /api/token/refresh/` `{refresh}` (в теле, legacy) | `{access}` |
| Профиль | `GET /user/` (Bearer access) | `id` → числовой `operator_id`, ФИО, `role` |

Станция — не браузер, поэтому клиент **извлекает refresh из заголовка
`Set-Cookie`** при входе и кэширует его строкой; обновление шлёт refresh в теле
(view это принимает). `ROTATE_REFRESH_TOKENS` в ex_system выключен → refresh
стабилен в пределах своего срока (24ч).

Реализация — [`sync::auth`](../src-tauri/src/sync/auth.rs): сеть спрятана за
trait-seam `AuthTransport` (боевой `HttpAuthTransport` на `reqwest::blocking`),
логика тестируется оффлайн. Ошибки категоризированы для UI: `InvalidCredentials`
(401/400), `Locked` (403), `Network` (обрыв/оффлайн), `Server` (5xx).

`server_base_url` берётся из `sync.server_base_url` (тот же адрес, что у
выгрузки/докета).

## Состояние сессии и команды

Активная сессия оператора живёт **в памяти ядра**
([`ipc::auth_cmds::AuthState`](../src-tauri/src/ipc/auth_cmds.rs), managed в
`lib.rs`). Команды Tauri:

- `auth_login(email, password, pin?)` — вход онлайн; профиль в шапку; сохраняет
  кэш-сессию; эмитит событие `auth_state`.
- `auth_unlock_offline(pin?)` — оффлайн-разблокировка по кэшу (окно + PIN).
- `auth_reconnect()` — тихий refresh при возврате онлайн (без действий оператора).
- `auth_logout()` — выход: чистит сессию **в памяти**; кэш оффлайн-сессии
  **сохраняется** (иначе «Сменить оператора» в оффлайн-зале лишил бы станцию
  повторного PIN-входа без связи; кэш перезапишется при следующем онлайн-входе
  или истечёт по окну). **Идущую запись не трогает**.
- `auth_status()` — снимок для шапки/гейта/экрана входа.

UI: экран `Login` ([`src/screens/Login.tsx`](../src/screens/Login.tsx)), общий
контекст `AuthProvider`/`useAuth`
([`src/lib/auth-context.tsx`](../src/lib/auth-context.tsx)), шапка `AppShell`
(ФИО + роль + индикатор связи + «Сменить оператора»), гейт маршрутов
`RequireOperator` в `App.tsx`.

## Кэш оффлайн-сессии

Формат кэша (решение заказчика) — **билет + refresh-токен**. Персист —
[`store::auth_cache`](../src-tauri/src/store/auth_cache.rs), отдельным
**всегда зашифрованным** блоб-файлом `auth_session.enc` (AES-256-GCM ключом
станции через `store::crypto` — тот же движок, что у сегментов/кэша дел). Состав:

```
CachedSession { operator_id, full_name, role, refresh_token,
                obtained_at_unix_ms, pin_salt, pin_hash }
```

Окно действия — `auth.operator.cached_session_hours` (дефолт 24ч, совпадает с
TTL refresh в `ex_system`): в окне разрешён оффлайн-старт; за окном — требуется
онлайн-вход. Возврат онлайн → `auth_reconnect`/планировщик выгрузки поднимают
свежий `access` по refresh-токену без действий оператора. Секреты (пароль,
access/refresh, PIN) в `settings.json` **не хранятся**.

## PIN (второй фактор оффлайн-разблокировки)

Решение заказчика — PIN включён. Политика — реестр:
`auth.operator.offline_pin.required` (дефолт `true`),
`auth.operator.offline_pin.min_length` (дефолт `4`). PIN задаётся на онлайн-входе
и хешируется Argon2id (случайная соль) в блоб кэш-сессии; при оффлайн-старте
`auth_unlock_offline` сверяет PIN с сохранённым хешем (константное по времени
сравнение). Сам PIN/хеш в `settings.json` не хранятся.

## Гейт старта и непрерывность записи

Гейт — [`ensure_start_allowed`](../src-tauri/src/ipc/auth_cmds.rs), вызывается в
`start_capture` после загрузки настроек: при
`auth.operator.required_to_start = true` без активного оператора старт **новой**
сессии отклоняется с понятным сообщением. Идущая запись `AuthState` не читает,
поэтому истечение токена, выход и смена оператора её не прерывают
(`auth.recording_survives_token_expiry`).

## Идентичность в данных

`operator_id` вошедшего оператора и `station_id` (учётка станции) проставляются
на старте в write-ahead журнал (`JournalRecord::SessionStarted`), доезжают через
реконсиляцию до манифеста (`store::reconcile`) и в регистрацию выгрузки
(`SessionMeta`, контракт `07`). Разметка/аудит доступа/экспорт берут автора из
`ipc::audio_cmds::operator_identity` (сессия входа).

## Тестовая подпорка (снята с боевого пути)

Env-переменные `COURT_AUDIO_OPERATOR_TOKEN` / `COURT_AUDIO_OPERATOR_ID` /
`COURT_AUDIO_STATION_ID` (`sync::mod`) остаются **только для тестов/CI и
legacy-сессий**. Боевой код токен выгрузки берёт из сессии входа
(`current_access_token`), `operator_id` — из `AuthState`; env читается лишь как
фолбэк при пустой идентичности (`#[cfg(test)]`/CI).

## Вне объёма (следующие этапы)

- Управление пользователями/ролями, парольные политики — `ex_system`.
- Разграничение настроек по ролям — этап `10.4` (использует роль из этого этапа).
- Своя учётка станции с отдельным входом (сейчас `station_id` — env-seam учётки
  станции); ГОСТ-подпись — этап `11`.
