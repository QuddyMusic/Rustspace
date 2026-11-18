//! Порт `packages/web/src/utils/identity.ts` (Backspace).
//!
//! Все клиентские мапы по людям должны ключеваться через [`user_key`], иначе
//! при мультиинстансности реплики одного и того же человека на разных origin
//! будут считаться разными сущностями.
//!
//! В вебе `origin` — это строка: `''` для домашнего инстанса, `'https://...'`
//! для удалённых. Здесь функции принимают `origin: &str` в том же формате
//! (Home = пустая строка) и дополнительно `local_host` — значение, которое в
//! вебе берётся из `window.location.host` (hostname с портом, если он есть).

use std::collections::HashMap;

use bs_types::User;

/// Разбор федеративного username.
///
/// `"erin@nova.ddns.net"` → `("erin", Some("nova.ddns.net"))`;
/// `"alice"` → `("alice", None)`. Пустая часть с любой стороны `@`
/// трактуется как отсутствие домена, возвращается исходная строка.
#[must_use]
pub fn parse_federated_username(username: &str) -> (&str, Option<&str>) {
    match username.split_once('@') {
        Some((base, domain)) if !base.is_empty() && !domain.is_empty() => (base, Some(domain)),
        _ => (username, None),
    }
}

/// Отображаемое имя с фолбэком на `baseName` из `username`.
#[must_use]
pub fn user_display_name(user: &User) -> String {
    if let Some(name) = user.display_name.as_deref().filter(|s| !s.is_empty()) {
        return name.to_owned();
    }
    parse_federated_username(&user.username).0.to_owned()
}

/// `new URL(origin).host` — hostname с портом (если он есть).
/// При ошибке разбора возвращает значение как есть.
#[must_use]
pub fn host_of(origin: &str) -> String {
    url::Url::parse(origin).map_or_else(|_| origin.to_owned(), |u| host_of_url(&u))
}

/// host:port из уже разобранного URL.
#[must_use]
pub fn host_of_url(url: &url::Url) -> String {
    url.host_str().map_or_else(String::new, |h| {
        url.port()
            .map_or_else(|| h.to_owned(), |p| format!("{h}:{p}"))
    })
}

/// Нормализация произвольной строки в origin-host (с портом).
///
/// Пусто → `""`. Если есть схема — берём `new URL(x).host`. Если схемы нет —
/// возвращаем строку как есть (в вебе порт без схемы уже считается частью
/// хоста и не отбрасывается).
#[must_use]
pub fn normalize_origin_to_host(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return String::new();
    }
    if s.contains("://") {
        url::Url::parse(s).map_or_else(|_| s.to_owned(), |u| host_of_url(&u))
    } else {
        s.to_owned()
    }
}

/// Хост, который реально доставил запись: `""` → `local_host`, иначе —
/// нормализованный host удалённого origin.
#[must_use]
pub fn delivering_host(origin: &str, local_host: &str) -> String {
    if origin.is_empty() {
        local_host.to_owned()
    } else {
        normalize_origin_to_host(origin)
    }
}

/// Bare hostname: без схемы, порта и пути; в нижнем регистре.
///
/// Сравнивать хосты между собой нужно только через эту функцию: сервер
/// отбрасывает порт и нечувствителен к регистру.
#[must_use]
pub fn home_host_of(value: &str) -> String {
    let s = value.trim();
    if s.is_empty() {
        return String::new();
    }
    let with_scheme = if s.contains("://") {
        s.to_owned()
    } else {
        format!("https://{s}")
    };
    if let Ok(u) = url::Url::parse(&with_scheme)
        && let Some(h) = u.host_str()
    {
        return h.to_ascii_lowercase();
    }
    // Fallback: ручное разделение, если URL не разобрался.
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let s = s.strip_prefix("//").unwrap_or(s);
    let s = s.split(['/', '?', '#']).next().unwrap_or("");
    let s = s.split(':').next().unwrap_or("");
    s.to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// Идентичность человека
// ---------------------------------------------------------------------------

/// Минимальный набор полей записи, нужный для вычисления ключа человека.
///
/// В TS это `Pick<User, 'id' | 'homeUserId' | 'homeInstance'>`; здесь —
/// отдельная структура, чтобы те же функции можно было применять к любым
/// репликам (участник пространства, автор сообщения и т.п.).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IdentityFields {
    /// Локальный id записи на инстансе-доставщике.
    pub id: String,
    /// Id на домашнем инстансе (у нативного пользователя — `None`).
    pub home_user_id: Option<String>,
    /// Домашний инстанс (у нативного пользователя — `None`).
    pub home_instance: Option<String>,
}

impl IdentityFields {
    /// Извлечь поля идентичности из [`User`].
    #[must_use]
    pub fn of_user(user: &User) -> Self {
        Self {
            id: user.id.clone(),
            home_user_id: user.home_user_id.clone(),
            home_instance: user.home_instance.clone(),
        }
    }
}

/// Домашняя идентичность человека: `(host, userId)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeIdentity {
    /// Bare hostname домашнего инстанса (см. [`home_host_of`]).
    pub host: String,
    /// id пользователя на домашнем инстансе.
    pub user_id: String,
}

/// `homeIdentityOf(row, origin)`: где запись «живёт по-настоящему».
///
/// Если у записи есть `homeInstance` — это реплика, и домашняя идентичность
/// берётся из `(homeInstance, homeUserId)`. Иначе запись нативная для
/// инстанса-доставщика и её домашняя идентичность — `(deliveringHost, row.id)`.
/// `None`, если запись реплицирована, но `homeUserId`/`homeInstance` пусты.
#[must_use]
pub fn home_identity_of(
    row: &IdentityFields,
    origin: &str,
    local_host: &str,
) -> Option<HomeIdentity> {
    match row.home_instance.as_deref() {
        Some(home) => {
            let host = normalize_origin_to_host(home);
            let user_id = row.home_user_id.clone()?;
            if host.is_empty() {
                return None;
            }
            Some(HomeIdentity { host, user_id })
        }
        None => Some(HomeIdentity {
            host: delivering_host(origin, local_host),
            user_id: row.id.clone(),
        }),
    }
}

/// Стабильный ключ человека: `<homeHost>:<homeUserId>`.
///
/// Для реплик одного человека на разных origin ключ совпадает — именно на
/// этом держатся все клиентские мапы по людям. Если идентичность не
/// определяется (legacy stub), возвращается уникальный локальный ключ
/// `~<host>:<rowId>`, чтобы запись не склеилась с чужой.
#[must_use]
pub fn user_key_of(row: &IdentityFields, origin: &str, local_host: &str) -> String {
    match home_identity_of(row, origin, local_host) {
        Some(identity) => format!("{}:{}", home_host_of(&identity.host), identity.user_id),
        None => format!(
            "~{}:{}",
            home_host_of(&delivering_host(origin, local_host)),
            row.id
        ),
    }
}

/// [`user_key_of`] для [`User`].
#[must_use]
pub fn user_key(user: &User, origin: &str, local_host: &str) -> String {
    user_key_of(&IdentityFields::of_user(user), origin, local_host)
}

/// Выдан ли пользователь своим домашним инстансом (а не реплицирован сюда).
#[must_use]
pub fn is_issued_by_home(row: &IdentityFields, origin: &str, local_host: &str) -> bool {
    home_identity_of(row, origin, local_host).is_some_and(|identity| {
        home_host_of(&identity.host) == home_host_of(&delivering_host(origin, local_host))
    })
}

/// Идентичность самого пользователя в мультиинстансном клиенте.
///
/// `row_ids` — карта `origin → rowId` (пустая строка = домашний инстанс):
/// на каждом инстансе у одного и того же человека свой локальный id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfIdentity {
    /// Ключ человека (см. [`user_key`]).
    pub key: String,
    /// `origin → rowId` для всех известных инстансов.
    pub row_ids: HashMap<String, String>,
}

impl SelfIdentity {
    /// Локальный id пользователя на данном origin, если он известен.
    #[must_use]
    pub fn row_id_at(&self, origin: &str) -> Option<&str> {
        self.row_ids.get(origin).map(String::as_str)
    }
}

/// `selfIdentityOf(user, myRowIds)`: `None`, если пользователь не залогинен.
// Конкретный `HashMap` важен: `SelfIdentity::row_ids` хранит именно его.
#[allow(clippy::implicit_hasher)]
#[must_use]
pub fn self_identity_of(
    user: Option<&User>,
    my_row_ids: &HashMap<String, String>,
    local_host: &str,
) -> Option<SelfIdentity> {
    let user = user?;
    let mut row_ids = my_row_ids.clone();
    row_ids.insert(String::new(), user.id.clone());
    Some(SelfIdentity {
        key: user_key(user, "", local_host),
        row_ids,
    })
}

/// `isMine(row, origin, self)`: принадлежит ли запись текущему пользователю.
///
/// Сначала пробуем точное совпадение по карте `origin → rowId` (это работает
/// и для legacy stub), затем — совпадение ключей людей.
#[must_use]
pub fn is_mine(
    row: &IdentityFields,
    origin: &str,
    local_host: &str,
    me: Option<&SelfIdentity>,
) -> bool {
    let Some(me) = me else {
        return false;
    };
    if me.row_id_at(origin) == Some(row.id.as_str()) {
        return true;
    }
    home_identity_of(row, origin, local_host).is_some()
        && user_key_of(row, origin, local_host) == me.key
}

/// `ownRowAt(session, origin, rowId)`: запись о себе для конкретного инстанса.
///
/// Нужна, когда в ответе сервера приходит «своя» запись с локальным id
/// другого инстанса: домашний инстанс сохраняет реальный id, удалённый —
/// ссылку на домашнюю идентичность.
#[must_use]
pub fn own_row_at(session: &User, origin: &str, row_id: &str, local_host: &str) -> User {
    let mut out = session.clone();
    row_id.clone_into(&mut out.id);
    let home = home_identity_of(&IdentityFields::of_user(session), "", local_host);
    let Some(home) = home else {
        return out;
    };
    if home_host_of(&home.host) == home_host_of(&delivering_host(origin, local_host)) {
        out.home_instance = None;
        out.home_user_id = None;
    } else {
        out.home_instance = Some(home.host);
        out.home_user_id = Some(home.user_id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: &str = "chat.example.com";

    fn user(id: &str, username: &str) -> User {
        User {
            id: id.to_owned(),
            username: username.to_owned(),
            display_name: None,
            avatar: None,
            banner: None,
            accent_color: None,
            avatar_color: None,
            bio: None,
            status: bs_types::UserStatus::Online,
            custom_status: None,
            is_admin: false,
            is_deleted: None,
            discoverable: None,
            profile_updated_at: None,
            created_at: 0,
            home_instance: None,
            home_user_id: None,
            replicated_instances: Vec::new(),
            show_activity: None,
            federation_home_orphaned: None,
            detached_home_instance: None,
            bot: false,
        }
    }

    fn fields(id: &str, home: Option<(&str, &str)>) -> IdentityFields {
        match home {
            Some((host, uid)) => IdentityFields {
                id: id.to_owned(),
                home_user_id: Some(uid.to_owned()),
                home_instance: Some(host.to_owned()),
            },
            None => IdentityFields {
                id: id.to_owned(),
                home_user_id: None,
                home_instance: None,
            },
        }
    }

    #[test]
    fn parses_federated_username_parts() {
        assert_eq!(
            parse_federated_username("erin@nova.ddns.net"),
            ("erin", Some("nova.ddns.net"))
        );
        assert_eq!(parse_federated_username("alice"), ("alice", None));
        assert_eq!(parse_federated_username("@host"), ("@host", None));
        assert_eq!(parse_federated_username("bob@"), ("bob@", None));
    }

    #[test]
    fn display_name_falls_back_to_base_name() {
        let mut u = user("1", "erin@nova.ddns.net");
        assert_eq!(user_display_name(&u), "erin");
        u.display_name = Some("Erin".to_owned());
        assert_eq!(user_display_name(&u), "Erin");
        u.display_name = Some(String::new());
        assert_eq!(user_display_name(&u), "erin");
    }

    #[test]
    fn home_host_drops_scheme_port_path_case() {
        assert_eq!(
            home_host_of("https://Nova.DDNS.net:8443/x"),
            "nova.ddns.net"
        );
        assert_eq!(home_host_of("nova.ddns.net"), "nova.ddns.net");
        assert_eq!(home_host_of("nova.ddns.net:8443"), "nova.ddns.net");
        assert_eq!(home_host_of(""), "");
    }

    #[test]
    fn normalize_origin_keeps_port_with_scheme() {
        assert_eq!(
            normalize_origin_to_host("https://nova.ddns.net:8443/path"),
            "nova.ddns.net:8443"
        );
        assert_eq!(
            normalize_origin_to_host("nova.ddns.net:8443"),
            "nova.ddns.net:8443"
        );
        assert_eq!(normalize_origin_to_host("  "), "");
    }

    #[test]
    fn delivering_host_uses_local_for_empty_origin() {
        assert_eq!(delivering_host("", LOCAL), LOCAL);
        assert_eq!(
            delivering_host("https://orbit.ddns.net", LOCAL),
            "orbit.ddns.net"
        );
    }

    #[test]
    fn home_identity_native_uses_delivering_host() {
        let row = fields("42", None);
        let id = home_identity_of(&row, "", LOCAL).expect("native has identity");
        assert_eq!(id.host, LOCAL);
        assert_eq!(id.user_id, "42");
    }

    #[test]
    fn home_identity_replicated_uses_home_fields() {
        let row = fields("7", Some(("orbit.ddns.net", "9")));
        let id = home_identity_of(&row, "https://nova.ddns.net", LOCAL).expect("replicated");
        assert_eq!(id.host, "orbit.ddns.net");
        assert_eq!(id.user_id, "9");
    }

    #[test]
    fn home_identity_none_without_home_user_id() {
        let row = IdentityFields {
            id: "7".to_owned(),
            home_user_id: None,
            home_instance: Some("orbit.ddns.net".to_owned()),
        };
        assert!(home_identity_of(&row, "", LOCAL).is_none());
    }

    #[test]
    fn user_key_matches_across_origins() {
        let at_home = user_key_of(&fields("42", None), "", LOCAL);
        let at_remote = user_key_of(
            &fields("7", Some((LOCAL, "42"))),
            "https://nova.ddns.net",
            LOCAL,
        );
        assert_eq!(at_home, format!("{LOCAL}:42"));
        assert_eq!(at_remote, at_home);
    }

    #[test]
    fn user_key_legacy_stub_is_unique_per_row() {
        let row = IdentityFields {
            id: "99".to_owned(),
            home_user_id: None,
            home_instance: Some("orbit.ddns.net".to_owned()),
        };
        let key = user_key_of(&row, "", LOCAL);
        assert!(key.starts_with('~'), "{key}");
        assert!(key.ends_with(":99"), "{key}");
        assert!(key.contains(LOCAL), "{key}");
    }

    #[test]
    fn issued_by_home_only_for_native() {
        assert!(is_issued_by_home(&fields("1", None), "", LOCAL));
        assert!(!is_issued_by_home(
            &fields("1", Some(("orbit.ddns.net", "1"))),
            "",
            LOCAL
        ));
    }

    #[test]
    fn self_identity_includes_local_row_id() {
        let me = user("42", "alice");
        let mut map = HashMap::new();
        map.insert("https://nova.ddns.net".to_owned(), "7".to_owned());
        let me = self_identity_of(Some(&me), &map, LOCAL).expect("logged in");
        assert_eq!(me.key, format!("{LOCAL}:42"));
        assert_eq!(me.row_id_at(""), Some("42"));
        assert_eq!(me.row_id_at("https://nova.ddns.net"), Some("7"));
        assert_eq!(me.row_id_at("https://other"), None);
        assert!(self_identity_of(None, &HashMap::new(), LOCAL).is_none());
    }

    #[test]
    fn is_mine_by_row_id_and_by_key() {
        let me_user = user("42", "alice");
        let me = self_identity_of(Some(&me_user), &HashMap::new(), LOCAL).unwrap();
        // Своя нативная запись.
        assert!(is_mine(&fields("42", None), "", LOCAL, Some(&me)));
        // Реплика себя на удалённом инстансе — другой rowId, тот же ключ.
        assert!(is_mine(
            &fields("7", Some((LOCAL, "42"))),
            "https://nova.ddns.net",
            LOCAL,
            Some(&me)
        ));
        // Чужой человек.
        assert!(!is_mine(&fields("43", None), "", LOCAL, Some(&me)));
        // Без self — всегда false.
        assert!(!is_mine(&fields("42", None), "", LOCAL, None));
    }

    #[test]
    fn own_row_at_home_clears_home_fields() {
        let me = user("42", "alice");
        let row = own_row_at(&me, "", "42", LOCAL);
        assert_eq!(row.id, "42");
        assert!(row.home_instance.is_none());
        assert!(row.home_user_id.is_none());
    }

    #[test]
    fn own_row_at_remote_sets_home_fields() {
        let me = user("42", "alice");
        let row = own_row_at(&me, "https://nova.ddns.net", "7", LOCAL);
        assert_eq!(row.id, "7");
        assert_eq!(row.home_instance.as_deref(), Some(LOCAL));
        assert_eq!(row.home_user_id.as_deref(), Some("42"));
    }
}
