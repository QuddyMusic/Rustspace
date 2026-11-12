//! Конфигурация из переменных окружения — те же имена и значения по
//! умолчанию, что у оригинала (`config.ts`), чтобы `.env` существующего
//! инстанса подходил без правок.

use std::collections::HashMap;
use std::path::PathBuf;

/// Версия оригинального сервера, с которой совместим этот (поле `version`
/// в `/api/instance/info`: по нему клиенты и пиры судят о возможностях).
pub const COMPAT_VERSION: &str = "1.11.0";
/// Репозиторий оригинала: значение `sourceCodeUrl` по умолчанию.
const UPSTREAM_SOURCE_URL: &str = "https://github.com/TheZwiss/backspace";
/// Сколько прокси перед сервером можно доверять самое большее.
const MAX_TRUSTED_PROXY_HOPS: u32 = 4;

/// Ошибка конфигурации: сервер с ней не стартует.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    /// Обязательная переменная не задана.
    #[error("Missing required environment variable: {0}")]
    Missing(&'static str),
    /// Значение не подходит.
    #[error("{0}")]
    Invalid(String),
}

/// Настройки LiveKit (все три нужны, чтобы работали звонки).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveKit {
    /// Адрес для клиентов.
    pub url: Option<String>,
    /// Ключ API.
    pub api_key: Option<String>,
    /// Секрет API.
    pub api_secret: Option<String>,
}

/// Конфигурация сервера.
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    /// Порт (`PORT`, 3000).
    pub port: u16,
    /// Адрес прослушивания (`HOST`, 0.0.0.0).
    pub host: String,
    /// Секрет подписи JWT (`JWT_SECRET`, не короче 32 символов).
    pub jwt_secret: String,
    /// Срок жизни токена в секундах (`JWT_EXPIRES_IN`, 30d).
    pub jwt_expires_secs: u64,
    /// Стоимость bcrypt (12; `BS_PASSWORD_HASH_COST` — только для тестов).
    pub password_hash_cost: u32,
    /// Домен инстанса (`DOMAIN`).
    pub domain: Option<String>,
    /// Публичный адрес (`PUBLIC_ORIGIN`).
    pub public_origin: Option<String>,
    /// Версия для `/api/instance/info`.
    pub version: String,
    /// Где лежит исходный код этого сервера (`BACKSPACE_SOURCE_URL`).
    pub source_code_url: String,
    /// Коммит сборки (`BACKSPACE_COMMIT`).
    pub commit: Option<String>,
    /// Можно ли по просьбе администратора узнавать о новом выпуске оригинала
    /// (`BACKSPACE_UPDATE_CHECK`, по умолчанию выключено).
    pub update_check: bool,
    /// Способ установки для панели обновлений (`BACKSPACE_INSTALL_CHANNEL`).
    pub install_channel: Option<String>,
    /// Скольким прокси доверять `X-Forwarded-For` (`TRUSTED_PROXY_HOPS`, 1).
    pub trusted_proxy_hops: u32,
    /// LiveKit.
    pub livekit: LiveKit,
    /// Каталог собранного веб-клиента (`BS_WEB_DIST`); без него сервер отдаёт
    /// только API.
    pub web_dist: Option<PathBuf>,
    /// Каталог загрузок (`UPLOAD_DIR`).
    pub upload_dir: PathBuf,
    /// Файл базы SQLite (`DB_PATH`).
    pub db_path: PathBuf,
    /// Адрес базы PostgreSQL (`DATABASE_URL`); без него сервер работает с
    /// файлом SQLite. В адресе есть пароль — в журнал он не выводится.
    pub database_url: Option<String>,
    /// Предел размера загрузки в байтах (`MAX_UPLOAD_SIZE`).
    pub max_upload_size: u64,
    /// Открыта ли регистрация, пока администратор не решил иначе
    /// (`REGISTRATION_OPEN`).
    pub registration_open: bool,
    /// Адрес каталога инстансов (`DIRECTORY_ENDPOINT`; пусто — выключен).
    pub directory_endpoint: String,
    /// Отключить ограничения частоты (`DISABLE_RATE_LIMITS`, для тестов).
    pub disable_rate_limits: bool,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Секреты в отладочный вывод не попадают.
        f.debug_struct("Config")
            .field("port", &self.port)
            .field("host", &self.host)
            .field("domain", &self.domain)
            .field("db_path", &self.db_path)
            .finish_non_exhaustive()
    }
}

/// Источник переменных (окружение процесса или набор для тестов).
struct Env<'a>(&'a HashMap<String, String>);

impl Env<'_> {
    fn raw(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// Как `envOptional`: пустая строка — «не задано».
    fn optional(&self, key: &str) -> Option<String> {
        self.raw(key).filter(|v| !v.is_empty()).map(str::to_owned)
    }

    fn string(&self, key: &str, default: &str) -> String {
        self.raw(key).unwrap_or(default).to_owned()
    }

    fn int<T: std::str::FromStr>(&self, key: &'static str, default: T) -> Result<T, ConfigError> {
        self.raw(key).map_or(Ok(default), |value| {
            value.trim().parse().map_err(|_| {
                ConfigError::Invalid(format!(
                    "Environment variable {key} must be a number, got: {value}"
                ))
            })
        })
    }

    fn flag(&self, key: &str, default: bool) -> bool {
        self.raw(key)
            .map_or(default, |value| value == "true" || value == "1")
    }
}

/// `30d`, `12h`, `45m`, `90s` или число секунд — как принимает jsonwebtoken.
fn parse_duration(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    let factor = match unit.trim() {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 604_800,
        "y" => 31_557_600,
        _ => return None,
    };
    number.checked_mul(factor).filter(|secs| *secs > 0)
}

fn check_http(name: &str, value: &str) -> Result<(), ConfigError> {
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(())
    } else {
        Err(ConfigError::Invalid(format!(
            "{name} must start with http:// or https:// — got: {value}"
        )))
    }
}

impl Config {
    /// Читает конфигурацию из окружения процесса.
    ///
    /// # Errors
    /// Не задан `JWT_SECRET` или значение переменной не подходит.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_map(&std::env::vars().collect())
    }

    /// Читает конфигурацию из набора переменных.
    ///
    /// # Errors
    /// Не задан `JWT_SECRET` или значение переменной не подходит.
    pub fn from_map(vars: &HashMap<String, String>) -> Result<Self, ConfigError> {
        let env = Env(vars);
        let jwt_secret = env
            .raw("JWT_SECRET")
            .ok_or(ConfigError::Missing("JWT_SECRET"))?
            .to_owned();
        if jwt_secret.chars().count() < 32 {
            return Err(ConfigError::Invalid(format!(
                "JWT_SECRET must be at least 32 characters (got {}). Generate one with: openssl rand -hex 32",
                jwt_secret.chars().count()
            )));
        }
        let expires = env.string("JWT_EXPIRES_IN", "30d");
        let jwt_expires_secs = parse_duration(&expires).ok_or_else(|| {
            ConfigError::Invalid(format!(
                "JWT_EXPIRES_IN must look like 30d, 12h, 45m or a number of seconds, got: {expires}"
            ))
        })?;
        let public_origin = env.optional("PUBLIC_ORIGIN");
        if let Some(origin) = &public_origin {
            check_http("PUBLIC_ORIGIN", origin)?;
        }
        let source_code_url = env
            .optional("BACKSPACE_SOURCE_URL")
            .unwrap_or_else(|| UPSTREAM_SOURCE_URL.to_owned());
        check_http("BACKSPACE_SOURCE_URL", &source_code_url)?;
        let trusted_proxy_hops: u32 = env.int("TRUSTED_PROXY_HOPS", 1)?;
        if trusted_proxy_hops > MAX_TRUSTED_PROXY_HOPS {
            return Err(ConfigError::Invalid(format!(
                "TRUSTED_PROXY_HOPS is how many proxies in front of this app may be trusted to have written \
                 X-Forwarded-For, and it is what every rate limit keys on. Got {trusted_proxy_hops}; the maximum \
                 is {MAX_TRUSTED_PROXY_HOPS}."
            )));
        }
        let password_hash_cost: u32 = env.int("BS_PASSWORD_HASH_COST", 12)?;
        if !(4..=16).contains(&password_hash_cost) {
            return Err(ConfigError::Invalid(format!(
                "BS_PASSWORD_HASH_COST must be between 4 and 16, got: {password_hash_cost}"
            )));
        }
        let upload_dir = PathBuf::from(env.string("UPLOAD_DIR", "data/uploads"));
        Ok(Self {
            port: env.int("PORT", 3000)?,
            host: env.string("HOST", "0.0.0.0"),
            jwt_secret,
            jwt_expires_secs,
            password_hash_cost,
            domain: env.optional("DOMAIN"),
            public_origin,
            version: COMPAT_VERSION.to_owned(),
            source_code_url,
            commit: env.optional("BACKSPACE_COMMIT"),
            update_check: env.flag("BACKSPACE_UPDATE_CHECK", false),
            install_channel: env.optional("BACKSPACE_INSTALL_CHANNEL"),
            trusted_proxy_hops,
            livekit: LiveKit {
                url: env.optional("LIVEKIT_URL"),
                api_key: env.optional("LIVEKIT_API_KEY"),
                api_secret: env.optional("LIVEKIT_API_SECRET"),
            },
            web_dist: env
                .optional("BS_WEB_DIST")
                .map(PathBuf::from)
                .filter(|dir| dir.join("index.html").is_file()),
            upload_dir,
            db_path: PathBuf::from(env.string("DB_PATH", "data/backspace.db")),
            database_url: env
                .optional("DATABASE_URL")
                .filter(|url| url.starts_with("postgres://") || url.starts_with("postgresql://")),
            max_upload_size: env.int("MAX_UPLOAD_SIZE", 104_857_600)?,
            registration_open: env.flag("REGISTRATION_OPEN", true),
            directory_endpoint: env
                .raw("DIRECTORY_ENDPOINT")
                .unwrap_or("https://explore.backspacechat.com")
                .trim()
                .trim_end_matches('/')
                .to_owned(),
            disable_rate_limits: env.flag("DISABLE_RATE_LIMITS", false),
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn defaults_match_original() {
        let config = Config::from_map(&vars(&[("JWT_SECRET", SECRET)])).expect("конфигурация");
        assert_eq!(config.port, 3000);
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.jwt_expires_secs, 30 * 86_400);
        assert_eq!(config.password_hash_cost, 12);
        assert!(config.registration_open);
        assert_eq!(config.trusted_proxy_hops, 1);
        assert_eq!(
            config.directory_endpoint,
            "https://explore.backspacechat.com"
        );
        assert_eq!(config.max_upload_size, 104_857_600);
    }

    #[test]
    fn secret_is_required_and_long_enough() {
        assert_eq!(
            Config::from_map(&vars(&[])),
            Err(ConfigError::Missing("JWT_SECRET"))
        );
        assert!(matches!(
            Config::from_map(&vars(&[("JWT_SECRET", "short")])),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn bad_values_are_rejected() {
        for (key, value) in [
            ("PORT", "abc"),
            ("PUBLIC_ORIGIN", "example.org"),
            ("TRUSTED_PROXY_HOPS", "9"),
            ("JWT_EXPIRES_IN", "soon"),
        ] {
            let result = Config::from_map(&vars(&[("JWT_SECRET", SECRET), (key, value)]));
            assert!(matches!(result, Err(ConfigError::Invalid(_))), "{key}");
        }
    }

    #[test]
    fn durations_parse_like_jsonwebtoken() {
        assert_eq!(parse_duration("30d"), Some(2_592_000));
        assert_eq!(parse_duration("12h"), Some(43_200));
        assert_eq!(parse_duration("3600"), Some(3600));
        assert_eq!(parse_duration("0"), None);
        assert_eq!(parse_duration("1 fortnight"), None);
    }

    #[test]
    fn debug_hides_secrets() {
        let config = Config::from_map(&vars(&[("JWT_SECRET", SECRET)])).expect("конфигурация");
        assert!(!format!("{config:?}").contains(SECRET));
    }
}
