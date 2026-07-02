//! Клиент аутентификации оператора через `ex_system` (этап 10.3 —
//! `promts/10_3_auth.md`, шаг 1).
//!
//! Снимает env-подпорку идентичности (`sync::OPERATOR_TOKEN_ENV` и др.): вход —
//! JWT-flow `ex_system` (`docs/auth.md` там же). Контур станции работает через
//! **cookie-flow** ex_system (`CookieTokenObtainPairView`): `POST /api/token/`
//! (`{email,password}`) отдаёт в теле только `{access}`, а **refresh кладёт в
//! httpOnly-cookie `ex_refresh`** (Path `/api/token/`). Десктоп не в браузере,
//! поэтому мы **извлекаем refresh из заголовка `Set-Cookie`** и кэшируем строкой;
//! обновление — `POST /api/token/refresh/` с refresh **в теле** (view принимает
//! legacy-тело). Профиль — `GET /user/` (`id` → числовой `operator_id`, ФИО, роль).
//!
//! Как и выгрузка ([`super::client`]), сеть спрятана за trait-seam
//! [`AuthTransport`]: логика входа/кэша/PIN тестируется оффлайн на фейке;
//! боевой [`HttpAuthTransport`] — тонкая обёртка на `reqwest::blocking`.
//!
//! **Кэш оффлайн-сессии** ([`CachedSession`]) — билет (`operator_id`/ФИО/роль) +
//! refresh-токен + хеш PIN (Argon2id); персистится зашифрованно в
//! [`crate::store::auth_cache`], действует в окне
//! `auth.operator.cached_session_hours`. Здесь — только чистые типы/логика без
//! файлового I/O (тестируемость).

use argon2::Argon2;
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Пара JWT-токенов из `POST /api/token/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
}

/// Профиль оператора из `GET /user/` (минимальный состав для станции).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorProfile {
    /// Числовой PK пользователя `ex_system` (строкой) — обязательный `operator_id`.
    pub operator_id: String,
    pub full_name: String,
    pub role: String,
}

/// Ошибка аутентификации с понятными для UI категориями (deliverable 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// Неверные логин/пароль (401/400 на входе).
    InvalidCredentials,
    /// Учётка заблокирована/нет доступа (403).
    Locked,
    /// Нет связи с сервером (обрыв/таймаут/DNS) — оффлайн.
    Network(String),
    /// Прочий серверный сбой (5xx/некорректный ответ).
    Server(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::InvalidCredentials => write!(f, "неверный логин или пароль"),
            AuthError::Locked => write!(f, "учётная запись заблокирована или нет доступа"),
            AuthError::Network(e) => write!(f, "нет связи с сервером: {e}"),
            AuthError::Server(e) => write!(f, "ошибка сервера аутентификации: {e}"),
        }
    }
}

impl std::error::Error for AuthError {}

/// Транспорт аутентификации (seam контракта `ex_system`). Реальная реализация —
/// [`HttpAuthTransport`]; в тестах — фейк.
pub trait AuthTransport: Send + Sync {
    /// Обменять логин/пароль на пару токенов (`POST /api/token/`).
    fn obtain_token(&self, email: &str, password: &str) -> Result<Tokens, AuthError>;
    /// Обновить access-токен по refresh (`POST /api/token/refresh/`).
    fn refresh(&self, refresh: &str) -> Result<String, AuthError>;
    /// Получить профиль оператора по access-токену (`GET /user/`).
    fn fetch_profile(&self, access: &str) -> Result<OperatorProfile, AuthError>;
}

// ── Кэш оффлайн-сессии ────────────────────────────────────────────────────────

/// Кэшированная сессия оператора для оффлайн-старта. Персистится **зашифрованно**
/// ([`crate::store::auth_cache`]); действует в окне `cached_session_hours`.
/// Содержит refresh-токен (тихий рефреш при возврате онлайн) и хеш PIN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedSession {
    pub operator_id: String,
    pub full_name: String,
    pub role: String,
    /// Refresh-токен `ex_system` (в блобе — под AES-256-GCM).
    pub refresh_token: String,
    /// Момент последнего успешного онлайн-входа (unix ms) — основа окна кэша.
    pub obtained_at_unix_ms: u64,
    /// Соль Argon2id для PIN (случайная, не секрет). Пуста, если PIN не задан.
    #[serde(default)]
    pub pin_salt: Vec<u8>,
    /// Хеш PIN (Argon2id по `pin_salt`). Пуст, если PIN не задан.
    #[serde(default)]
    pub pin_hash: Vec<u8>,
}

impl CachedSession {
    /// Профиль оператора из кэша (для UI/идентичности).
    pub fn profile(&self) -> OperatorProfile {
        OperatorProfile {
            operator_id: self.operator_id.clone(),
            full_name: self.full_name.clone(),
            role: self.role.clone(),
        }
    }
}

/// Валиден ли кэш относительно окна `cached_session_hours` (реестр). Чистая
/// функция (тест): `now - obtained <= hours`.
pub fn cached_session_valid(obtained_at_unix_ms: u64, now_unix_ms: u64, hours: u32) -> bool {
    let window_ms = (hours as u64).saturating_mul(60 * 60 * 1000);
    now_unix_ms.saturating_sub(obtained_at_unix_ms) <= window_ms
}

/// Момент истечения кэша (unix ms) — для UI-индикации окна оффлайн-старта.
pub fn cache_expires_at_unix_ms(obtained_at_unix_ms: u64, hours: u32) -> u64 {
    let window_ms = (hours as u64).saturating_mul(60 * 60 * 1000);
    obtained_at_unix_ms.saturating_add(window_ms)
}

// ── PIN (второй фактор оффлайн-разблокировки) ─────────────────────────────────

/// Длина соли PIN — 128 бит (крипто-константа Argon2id, как в `store::crypto`).
const PIN_SALT_LEN: usize = 16;
/// Длина хеша PIN — 256 бит (Argon2id).
const PIN_HASH_LEN: usize = 32;

/// Захешировать PIN: случайная соль + Argon2id → `(salt, hash)`. Параметры KDF —
/// рекомендованные дефолты крейта (крипто-константы, не бизнес-логика).
pub fn hash_pin(pin: &str) -> (Vec<u8>, Vec<u8>) {
    let mut salt = vec![0u8; PIN_SALT_LEN];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    let hash = derive_pin(pin, &salt);
    (salt, hash)
}

/// Проверить PIN против сохранённых соли/хеша (константное по времени сравнение).
pub fn verify_pin(pin: &str, salt: &[u8], expected_hash: &[u8]) -> bool {
    if salt.is_empty() || expected_hash.is_empty() {
        return false;
    }
    let got = derive_pin(pin, salt);
    constant_time_eq(&got, expected_hash)
}

/// Argon2id(pin, salt) → 32 байта. Пустой результат при ошибке KDF (обрабатываем
/// как несовпадение выше).
fn derive_pin(pin: &str, salt: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; PIN_HASH_LEN];
    match Argon2::default().hash_password_into(pin.as_bytes(), salt, &mut out) {
        Ok(()) => out,
        Err(_) => Vec::new(),
    }
}

/// Сравнение байт за постоянное (относительно длины) время — без раннего выхода.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ── Боевой HTTP-транспорт ─────────────────────────────────────────────────────

/// Имя refresh-cookie ex_system (`CookieTokenObtainPairView`). refresh
/// не приходит в теле — только в `Set-Cookie` (httpOnly, Path `/api/token/`).
const REFRESH_COOKIE_NAME: &str = "ex_refresh";

/// Тело ответа входа/refresh: только `access` (refresh — в cookie).
#[derive(Debug, Deserialize)]
struct AccessResponse {
    access: String,
}

/// Извлечь значение cookie `name` из заголовков `Set-Cookie` ответа. Формат
/// заголовка: `name=value; Path=…; HttpOnly; …` — берём часть до первой `;`.
fn extract_set_cookie(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    for value in headers.get_all(reqwest::header::SET_COOKIE).iter() {
        let raw = value.to_str().ok()?;
        if let Some(rest) = raw.strip_prefix(&prefix) {
            let token = rest.split(';').next().unwrap_or("").trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// Ответ `GET /user/` — берём `id` и, что есть, для ФИО/роли (мягкая деградация).
#[derive(Debug, Deserialize)]
struct UserResponse {
    id: i64,
    #[serde(default)]
    full_name: Option<String>,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

impl UserResponse {
    fn into_profile(self) -> OperatorProfile {
        let full_name = self
            .full_name
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                let combined = format!(
                    "{} {}",
                    self.last_name.unwrap_or_default(),
                    self.first_name.unwrap_or_default()
                );
                combined.trim().to_string()
            });
        OperatorProfile {
            operator_id: self.id.to_string(),
            full_name,
            role: self.role.unwrap_or_default(),
        }
    }
}

/// Боевой транспорт входа на `reqwest::blocking`. Базовый URL —
/// `sync.server_base_url`.
pub struct HttpAuthTransport {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl HttpAuthTransport {
    /// Создать клиент для базового URL `ex_system`.
    pub fn new(base_url: impl Into<String>) -> Result<Self, AuthError> {
        let client = reqwest::blocking::Client::builder()
            .build()
            .map_err(|e| AuthError::Network(format!("инициализация HTTP-клиента: {e}")))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }
}

/// Классифицировать HTTP-статус ответа авторизации в [`AuthError`].
fn auth_status_error(status: reqwest::StatusCode) -> AuthError {
    match status.as_u16() {
        401 | 400 => AuthError::InvalidCredentials,
        403 => AuthError::Locked,
        s if (500..600).contains(&s) => AuthError::Server(format!("сервер вернул {status}")),
        _ => AuthError::Server(format!("неожиданный ответ сервера {status}")),
    }
}

impl AuthTransport for HttpAuthTransport {
    fn obtain_token(&self, email: &str, password: &str) -> Result<Tokens, AuthError> {
        let resp = self
            .client
            .post(self.url("api/token/"))
            .json(&serde_json::json!({ "email": email, "password": password }))
            .send()
            .map_err(|e| AuthError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(auth_status_error(resp.status()));
        }
        // refresh — из httpOnly-cookie `Set-Cookie` (в теле его нет). Пустой —
        // не ошибка входа: онлайн-сессия работает, но тихий refresh/оффлайн будут
        // недоступны (кэшировать нечего).
        let refresh = extract_set_cookie(resp.headers(), REFRESH_COOKIE_NAME).unwrap_or_default();
        let parsed: AccessResponse = resp
            .json()
            .map_err(|e| AuthError::Server(format!("разбор ответа входа: {e}")))?;
        Ok(Tokens {
            access: parsed.access,
            refresh,
        })
    }

    fn refresh(&self, refresh: &str) -> Result<String, AuthError> {
        // ROTATE_REFRESH_TOKENS выключен в ex_system → refresh стабилен, шлём его
        // в теле (CookieTokenRefreshView принимает legacy-тело помимо cookie).
        let resp = self
            .client
            .post(self.url("api/token/refresh/"))
            .json(&serde_json::json!({ "refresh": refresh }))
            .send()
            .map_err(|e| AuthError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(auth_status_error(resp.status()));
        }
        let parsed: AccessResponse = resp
            .json()
            .map_err(|e| AuthError::Server(format!("разбор ответа refresh: {e}")))?;
        Ok(parsed.access)
    }

    fn fetch_profile(&self, access: &str) -> Result<OperatorProfile, AuthError> {
        let resp = self
            .client
            .get(self.url("user/"))
            .bearer_auth(access)
            .send()
            .map_err(|e| AuthError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(auth_status_error(resp.status()));
        }
        let parsed: UserResponse = resp
            .json()
            .map_err(|e| AuthError::Server(format!("разбор профиля пользователя: {e}")))?;
        Ok(parsed.into_profile())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const HOUR_MS: u64 = 60 * 60 * 1000;

    #[test]
    fn cached_session_window() {
        let obtained = 1_000_000_000_000u64;
        // Ровно в окне 24ч.
        assert!(cached_session_valid(obtained, obtained + 24 * HOUR_MS, 24));
        // На секунду за окном — невалиден.
        assert!(!cached_session_valid(
            obtained,
            obtained + 24 * HOUR_MS + 1000,
            24
        ));
        // «Сейчас» до входа (перевод часов назад) — saturating, считаем валидным.
        assert!(cached_session_valid(obtained, obtained - 5000, 24));
        assert_eq!(
            cache_expires_at_unix_ms(obtained, 24),
            obtained + 24 * HOUR_MS
        );
    }

    #[test]
    fn pin_hash_verifies_correct_and_rejects_wrong() {
        let (salt, hash) = hash_pin("2468");
        assert!(verify_pin("2468", &salt, &hash));
        assert!(!verify_pin("0000", &salt, &hash));
        // Пустые соль/хеш (PIN не задан) — всегда отказ.
        assert!(!verify_pin("2468", &[], &[]));
        // Соль случайна: другой вызов даёт другой хеш.
        let (salt2, hash2) = hash_pin("2468");
        assert_ne!(salt, salt2);
        assert_ne!(hash, hash2);
        assert!(verify_pin("2468", &salt2, &hash2));
    }

    /// Фейк-транспорт: программируемые ответы, оффлайн (без сети).
    struct FakeAuth {
        tokens: Result<Tokens, AuthError>,
        refreshed: Result<String, AuthError>,
        profile: Result<OperatorProfile, AuthError>,
        calls: Mutex<Vec<String>>,
    }

    impl AuthTransport for FakeAuth {
        fn obtain_token(&self, email: &str, _password: &str) -> Result<Tokens, AuthError> {
            self.calls.lock().unwrap().push(format!("obtain:{email}"));
            self.tokens.clone()
        }
        fn refresh(&self, _refresh: &str) -> Result<String, AuthError> {
            self.calls.lock().unwrap().push("refresh".into());
            self.refreshed.clone()
        }
        fn fetch_profile(&self, _access: &str) -> Result<OperatorProfile, AuthError> {
            self.calls.lock().unwrap().push("profile".into());
            self.profile.clone()
        }
    }

    #[test]
    fn fake_transport_login_flow() {
        let fake = FakeAuth {
            tokens: Ok(Tokens {
                access: "a".into(),
                refresh: "r".into(),
            }),
            refreshed: Ok("a2".into()),
            profile: Ok(OperatorProfile {
                operator_id: "42".into(),
                full_name: "Иванов И. И.".into(),
                role: "assistant".into(),
            }),
            calls: Mutex::new(Vec::new()),
        };
        let t = fake.obtain_token("op@court", "pw").unwrap();
        assert_eq!(t.access, "a");
        let p = fake.fetch_profile(&t.access).unwrap();
        assert_eq!(p.operator_id, "42");
        assert_eq!(fake.refresh(&t.refresh).unwrap(), "a2");
    }

    #[test]
    fn error_mapping_categories() {
        assert_eq!(auth_status_error(reqwest::StatusCode::UNAUTHORIZED), AuthError::InvalidCredentials);
        assert_eq!(auth_status_error(reqwest::StatusCode::BAD_REQUEST), AuthError::InvalidCredentials);
        assert_eq!(auth_status_error(reqwest::StatusCode::FORBIDDEN), AuthError::Locked);
        assert!(matches!(
            auth_status_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            AuthError::Server(_)
        ));
    }

    #[test]
    fn user_response_builds_full_name_from_parts() {
        let u = UserResponse {
            id: 7,
            full_name: None,
            first_name: Some("Иван".into()),
            last_name: Some("Иванов".into()),
            role: Some("judge".into()),
        };
        let p = u.into_profile();
        assert_eq!(p.operator_id, "7");
        assert_eq!(p.full_name, "Иванов Иван");
        assert_eq!(p.role, "judge");
    }

    #[test]
    fn extracts_refresh_from_set_cookie() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            reqwest::header::SET_COOKIE,
            "ex_refresh=eyJhbGciOi.abc.def; Path=/api/token/; HttpOnly; SameSite=Lax"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            extract_set_cookie(&headers, REFRESH_COOKIE_NAME),
            Some("eyJhbGciOi.abc.def".to_string())
        );
        // Нет нужной cookie → None.
        assert_eq!(extract_set_cookie(&headers, "other"), None);
    }

    #[test]
    fn http_transport_url_join_is_clean() {
        let t = HttpAuthTransport::new("https://ex.example/").unwrap();
        assert_eq!(t.url("/api/token/"), "https://ex.example/api/token/");
        assert_eq!(t.url("user/"), "https://ex.example/user/");
    }
}
