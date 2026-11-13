//! Сквозное шифрование личных переписок на стороне клиента.
//!
//! Шифрует и расшифровывает [`bs_e2ee`]; здесь — что хранится на устройстве
//! и как ключи собеседников попадают в клиент:
//! - секретный ключ аккаунта лежит в системном хранилище ключей (Secret
//!   Service, Keychain, Credential Manager), а без него — в файле рядом с
//!   сохранённой сессией (права `0600`), где всегда лежат настройки переписок;
//! - ключ переносится на другое устройство записью, защищённой паролем;
//! - открытый ключ собеседника приходит сообщением прямо в переписке или из
//!   справочника сервера и запоминается при первом знакомстве; если позже
//!   приходит другой ключ, шифрование этой переписки останавливается, пока
//!   пользователь не подтвердит новый ключ.
//!
//! Шифруются только переписки один на один и только текст: вложения и
//! групповые переписки идут как раньше.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use bs_e2ee::{PublicKey, SecretKey};
use bs_types::{DmChannel, DmChannelLastMessage, DmMessage, DmMessageWithUser};
use serde::{Deserialize, Serialize};

use crate::i18n::tr;

/// Настройки одной переписки.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
struct Chat {
    /// Исходящие сообщения шифруются.
    enabled: bool,
    /// Запомненный ключ собеседника.
    peer_key: Option<String>,
    /// Время сообщения, из которого взят ключ (0 — из справочника).
    peer_key_at: i64,
    /// Новый ключ собеседника, ещё не подтверждённый пользователем.
    pending_key: Option<String>,
    /// Время сообщения с новым ключом.
    pending_key_at: i64,
    /// Прежние ключи собеседника: ими читается старая история.
    old_keys: Vec<String>,
    /// Свой ключ в эту переписку уже отправлен.
    announced: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Stored {
    /// Ключ, если он не в системном хранилище.
    secret_key: String,
    /// Ключ лежит в системном хранилище.
    key_in_keyring: bool,
    chats: HashMap<String, Chat>,
}

/// Что показать пользователю о шифровании переписки.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ChatStatus {
    /// Исходящие шифруются.
    pub(crate) enabled: bool,
    /// Ключ собеседника известен.
    pub(crate) has_peer_key: bool,
    /// Собеседник сменил ключ, и новый не подтверждён.
    pub(crate) key_changed: bool,
    /// Контрольное число пары ключей.
    pub(crate) safety_number: Option<String>,
}

/// Шифрование переписок одного аккаунта на этом устройстве.
pub(crate) struct E2ee {
    path: Option<PathBuf>,
    me: String,
    key: SecretKey,
    chats: HashMap<String, Chat>,
    /// Свои ключи, которые надо отправить в переписки: id переписки и текст.
    pub(crate) outbox: Vec<(String, String)>,
    /// Ключ лежит в системном хранилище, но сейчас его оттуда не взять:
    /// работает временный ключ, а сохранённое состояние не трогается.
    locked: bool,
}

impl std::fmt::Debug for E2ee {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2ee")
            .field("me", &self.me)
            .field("chats", &self.chats.len())
            .finish_non_exhaustive()
    }
}

/// Файл состояния рядом с файлом сохранённой сессии.
fn state_path(session_file: &Path, me: &str) -> PathBuf {
    let safe: String = me
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    session_file.with_file_name(format!("e2ee-{safe}.json"))
}

fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let written = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(body)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

impl E2ee {
    /// Загружает состояние аккаунта или создаёт новое с новым ключом.
    /// `session_file` — файл сохранённой сессии; без него ключ живёт только
    /// до конца работы программы.
    pub(crate) fn load(session_file: Option<&Path>, me: &str) -> Self {
        let path = session_file.map(|file| state_path(file, me));
        let stored: Option<Stored> = path
            .as_deref()
            .and_then(|path| std::fs::read(path).ok())
            .map(zeroize::Zeroizing::new)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let in_keyring = stored.as_ref().is_some_and(|stored| stored.key_in_keyring);
        let restored = match (&stored, &path) {
            (Some(_), Some(path)) if in_keyring => {
                crate::saved_session::keystore::get(&keyring_account(path))
                    .and_then(|backup| SecretKey::from_backup(&backup))
            }
            (Some(stored), _) => SecretKey::from_backup(&stored.secret_key),
            (None, _) => None,
        };
        // Ключ в хранилище есть, но хранилище недоступно: новый ключ здесь
        // заменил бы ключ аккаунта у всех собеседников, поэтому он временный.
        let locked = in_keyring && restored.is_none();
        if locked {
            tracing::warn!("ключ шифрования переписок в системном хранилище недоступен");
        }
        let fresh = restored.is_none();
        let state = Self {
            path: if locked { None } else { path },
            me: me.to_owned(),
            key: restored.unwrap_or_else(SecretKey::generate),
            // С новым ключом прежние настройки переписок теряют смысл.
            chats: stored
                .filter(|_| !fresh)
                .map(|stored| stored.chats)
                .unwrap_or_default(),
            outbox: Vec::new(),
            locked,
        };
        if fresh {
            state.save();
        }
        state
    }

    /// Ключ из хранилища недоступен (работает временный).
    pub(crate) const fn locked(&self) -> bool {
        self.locked
    }

    /// Запись ключа для другого устройства, защищённая паролем.
    ///
    /// # Errors
    /// Текст для пользователя.
    pub(crate) fn export_key(&self, passphrase: &str) -> Result<String, String> {
        bs_e2ee::export_key(&self.key, passphrase).map_err(transfer_error)
    }

    /// Ставит ключ с другого устройства вместо своего. Собеседники, которые
    /// знают этот ключ, смены не увидят; включённые переписки получают его
    /// объявление заново.
    ///
    /// # Errors
    /// Текст для пользователя.
    pub(crate) fn import_key(&mut self, text: &str, passphrase: &str) -> Result<(), String> {
        let key = bs_e2ee::import_key(text, passphrase).map_err(transfer_error)?;
        if key.public() == self.key.public() {
            return Ok(());
        }
        self.key = key;
        self.locked = false;
        let mut enabled = Vec::new();
        for (dm_id, chat) in &mut self.chats {
            chat.announced = false;
            if chat.enabled {
                enabled.push(dm_id.clone());
            }
        }
        enabled.sort();
        for dm_id in enabled {
            self.announce(&dm_id);
        }
        self.save();
        Ok(())
    }

    fn save(&self) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let backup = self.key.to_backup();
        let in_keyring = crate::saved_session::keystore::set(&keyring_account(path), &backup);
        let stored = Stored {
            secret_key: if in_keyring {
                String::new()
            } else {
                backup.to_string()
            },
            key_in_keyring: in_keyring,
            chats: self.chats.clone(),
        };
        let body = serde_json::to_vec_pretty(&stored).map(zeroize::Zeroizing::new);
        let written = body
            .map_err(std::io::Error::other)
            .and_then(|body| write_private(path, &body));
        if let Err(err) = written {
            tracing::warn!(%err, "не удалось сохранить ключи шифрования переписок");
        }
    }

    /// Свой открытый ключ в записи для передачи.
    pub(crate) fn public_key(&self) -> String {
        self.key.public().to_wire()
    }

    fn peer(chat: &Chat) -> Option<PublicKey> {
        chat.peer_key.as_deref().and_then(PublicKey::from_wire)
    }

    /// Состояние шифрования переписки.
    pub(crate) fn status(&self, dm_id: &str) -> ChatStatus {
        let Some(chat) = self.chats.get(dm_id) else {
            return ChatStatus::default();
        };
        let peer = Self::peer(chat);
        ChatStatus {
            enabled: chat.enabled,
            has_peer_key: peer.is_some(),
            key_changed: chat.pending_key.is_some(),
            safety_number: peer.map(|peer| bs_e2ee::safety_number(&self.key.public(), &peer)),
        }
    }

    fn announce(&mut self, dm_id: &str) {
        let key = self.key.public();
        let chat = self.chats.entry(dm_id.to_owned()).or_default();
        if !chat.announced {
            chat.announced = true;
            self.outbox
                .push((dm_id.to_owned(), bs_e2ee::key_announcement(&key)));
        }
    }

    /// Включает или выключает шифрование исходящих в переписке. При
    /// включении собеседнику один раз отправляется свой открытый ключ.
    pub(crate) fn set_enabled(&mut self, dm_id: &str, enabled: bool) {
        self.chats.entry(dm_id.to_owned()).or_default().enabled = enabled;
        if enabled {
            self.announce(dm_id);
        }
        self.save();
    }

    /// Пользователь сверил и принял новый ключ собеседника.
    pub(crate) fn accept_new_key(&mut self, dm_id: &str) {
        if let Some(chat) = self.chats.get_mut(dm_id)
            && let Some(key) = chat.pending_key.take()
        {
            if let Some(previous) = chat.peer_key.replace(key) {
                chat.old_keys.push(previous);
            }
            chat.peer_key_at = chat.peer_key_at.max(chat.pending_key_at);
            self.save();
        }
    }

    /// Учитывает ключ собеседника, полученный в момент `at` (0 — из
    /// справочника сервера). Возвращает, изменилось ли что-то.
    fn learn_peer_key(&mut self, dm_id: &str, key: PublicKey, at: i64) -> bool {
        let wire = key.to_wire();
        let chat = self.chats.entry(dm_id.to_owned()).or_default();
        // Старое объявление из истории не отменяет более новый ключ.
        if at != 0 && at < chat.peer_key_at {
            return false;
        }
        if chat.peer_key.as_deref() == Some(wire.as_str()) {
            let cleared = chat.pending_key.take().is_some();
            chat.peer_key_at = chat.peer_key_at.max(at);
            return cleared;
        }
        if chat.peer_key.is_none() {
            chat.peer_key = Some(wire);
            chat.peer_key_at = at;
            return true;
        }
        if chat.pending_key.as_deref() == Some(wire.as_str()) {
            return false;
        }
        chat.pending_key = Some(wire);
        chat.pending_key_at = at;
        true
    }

    /// Ключ собеседника из справочника сервера.
    pub(crate) fn directory_key(&mut self, dm_id: &str, key: &str) {
        if let Some(key) = PublicKey::from_wire(key)
            && self.learn_peer_key(dm_id, key, 0)
        {
            self.save();
        }
    }

    /// Обрабатывает входящее личное сообщение: объявление ключа запоминает,
    /// зашифрованный текст заменяет открытым. `direct` — переписка один на один.
    fn incoming(&mut self, message: &mut DmMessage, direct: bool) {
        let Some(content) = message.content.as_deref() else {
            return;
        };
        if let Some(key) = bs_e2ee::parse_key_announcement(content) {
            let mine = message.user_id.as_str() == self.me;
            if direct && !mine {
                let dm_id = message.dm_channel_id.clone();
                let changed = self.learn_peer_key(&dm_id, key, message.created_at);
                // Собеседник включил шифрование: отвечаем своим ключом и
                // шифруем ответы, чтобы переписка не шла открыто в одну сторону.
                let chat = self.chats.entry(dm_id.clone()).or_default();
                let first = !chat.announced;
                if first {
                    chat.enabled = true;
                    self.announce(&dm_id);
                }
                if changed || first {
                    self.save();
                }
            }
            message.content = Some(
                tr(if mine {
                    "Вы передали ключ шифрования"
                } else {
                    "Собеседник передал ключ шифрования"
                })
                .to_owned(),
            );
            return;
        }
        if !bs_e2ee::is_sealed(content) {
            return;
        }
        let peer = self.chats.get(&message.dm_channel_id).filter(|_| direct);
        let keys = peer.into_iter().flat_map(|chat| {
            chat.peer_key
                .iter()
                .chain(&chat.pending_key)
                .chain(&chat.old_keys)
                .map(String::as_str)
        });
        let opened = keys
            .filter_map(PublicKey::from_wire)
            .find_map(|peer| bs_e2ee::open(&self.key, &peer, content).ok());
        message.content =
            Some(opened.unwrap_or_else(|| tr("Не удалось расшифровать сообщение").to_owned()));
    }

    /// Сообщение с ответом внутри.
    pub(crate) fn incoming_full(&mut self, message: &mut DmMessageWithUser, direct: bool) {
        self.incoming(&mut message.message, direct);
        if let Some(reply) = message.reply_to.as_deref_mut() {
            self.incoming_full(reply, direct);
        }
    }

    /// Страница истории: от старых сообщений к новым, чтобы ключи
    /// запоминались в том порядке, в каком объявлялись.
    pub(crate) fn incoming_page(&mut self, page: &mut [DmMessageWithUser], direct: bool) {
        let mut order: Vec<usize> = (0..page.len()).collect();
        order.sort_by_key(|index| page[*index].message.created_at);
        for index in order {
            self.incoming_full(&mut page[index], direct);
        }
    }

    /// Последнее сообщение в списке переписок.
    pub(crate) fn incoming_channel(&mut self, channel: &mut DmChannel) {
        let direct = is_direct(channel);
        match channel.last_message.as_mut() {
            Some(DmChannelLastMessage::Full(message)) => self.incoming_full(message, direct),
            Some(DmChannelLastMessage::Preview(preview)) => {
                let sealed = preview.content.as_deref().is_some_and(|text| {
                    bs_e2ee::is_sealed(text) || bs_e2ee::parse_key_announcement(text).is_some()
                });
                if sealed {
                    preview.content = Some(tr("Зашифрованное сообщение").to_owned());
                }
            }
            None => {}
        }
    }

    /// Готовит исходящий текст: шифрует, если для переписки это включено.
    ///
    /// # Errors
    /// Текст для пользователя: почему сообщение не отправлено.
    pub(crate) fn outgoing(&self, dm_id: &str, content: String) -> Result<String, String> {
        let Some(chat) = self.chats.get(dm_id).filter(|chat| chat.enabled) else {
            return Ok(content);
        };
        if chat.pending_key.is_some() {
            return Err(tr("Собеседник сменил ключ шифрования: сверьте контрольное число и подтвердите новый ключ").to_owned());
        }
        let Some(peer) = Self::peer(chat) else {
            return Err(tr("Ключ собеседника ещё не получен: сообщение не отправлено").to_owned());
        };
        bs_e2ee::seal(&self.key, &peer, &content).map_err(|err| match err {
            bs_e2ee::SealError::TooLong => {
                tr("Сообщение слишком длинное для шифрования").to_owned()
            }
            bs_e2ee::SealError::BadPeerKey => {
                tr("Ключ собеседника негоден: шифрование невозможно").to_owned()
            }
        })
    }
}

/// Запись ключа в системном хранилище: по файлу состояния.
fn keyring_account(path: &Path) -> String {
    format!("e2ee:{}", path.display())
}

/// Ошибка переноса ключа для пользователя.
fn transfer_error(error: bs_e2ee::TransferError) -> String {
    tr(match error {
        bs_e2ee::TransferError::WeakPassphrase => "Пароль должен быть не короче 8 знаков",
        bs_e2ee::TransferError::Malformed => "Это не запись ключа шифрования или она повреждена",
        bs_e2ee::TransferError::Rejected => "Пароль не подходит к этой записи ключа",
    })
    .to_owned()
}

/// Переписка один на один (не группа).
pub(crate) fn is_direct(channel: &DmChannel) -> bool {
    channel.owner_id.is_none() && channel.members.len() == 2
}

#[cfg(test)]
#[path = "e2ee_live.rs"]
#[allow(clippy::expect_used, clippy::panic, reason = "тест на живых серверах")]
mod live;

#[cfg(test)]
mod tests {
    use super::*;

    fn message(dm: &str, author: &str, at: i64, content: &str) -> DmMessage {
        serde_json::from_value(serde_json::json!({
            "id": format!("m{at}"), "dmChannelId": dm, "userId": author, "content": content, "createdAt": at,
        }))
        .unwrap_or_else(|err| panic!("{err}"))
    }

    /// Доставляет исходящие объявления ключей одной стороны другой.
    fn deliver(from: &mut E2ee, to: &mut E2ee, at: i64) {
        for (dm, content) in std::mem::take(&mut from.outbox) {
            let mut incoming = message(&dm, &from.me, at, &content);
            to.incoming(&mut incoming, true);
        }
    }

    #[test]
    fn two_clients_agree_on_keys_and_read_each_other() {
        let (mut ann, mut bob) = (E2ee::load(None, "ann"), E2ee::load(None, "bob"));
        assert_eq!(
            ann.outgoing("dm", "открыто".to_owned()).as_deref(),
            Ok("открыто")
        );
        ann.set_enabled("dm", true);
        assert!(
            ann.outgoing("dm", "рано".to_owned()).is_err(),
            "ключа собеседника ещё нет"
        );
        deliver(&mut ann, &mut bob, 10);
        assert!(bob.status("dm").enabled, "ответная сторона включается сама");
        deliver(&mut bob, &mut ann, 11);
        assert!(
            ann.outbox.is_empty() && bob.outbox.is_empty(),
            "обмен ключами не зацикливается"
        );
        assert_eq!(
            ann.status("dm").safety_number,
            bob.status("dm").safety_number
        );
        assert!(ann.status("dm").safety_number.is_some());

        let sealed = ann.outgoing("dm", "секрет".to_owned()).unwrap_or_default();
        assert!(bs_e2ee::is_sealed(&sealed));
        let mut at_bob = message("dm", "ann", 12, &sealed);
        bob.incoming(&mut at_bob, true);
        assert_eq!(at_bob.content.as_deref(), Some("секрет"));
        let mut own_copy = message("dm", "ann", 12, &sealed);
        ann.incoming(&mut own_copy, true);
        assert_eq!(own_copy.content.as_deref(), Some("секрет"));
    }

    #[test]
    fn changed_peer_key_stops_encryption_until_accepted() {
        let (mut ann, mut bob) = (E2ee::load(None, "ann"), E2ee::load(None, "bob"));
        ann.set_enabled("dm", true);
        deliver(&mut ann, &mut bob, 10);
        deliver(&mut bob, &mut ann, 11);
        let before = ann.status("dm").safety_number;
        // Кто-то (или сам собеседник с нового устройства) объявляет другой ключ.
        let mut other = E2ee::load(None, "bob");
        other.set_enabled("dm", true);
        deliver(&mut other, &mut ann, 20);
        let status = ann.status("dm");
        assert!(status.key_changed);
        assert_eq!(
            status.safety_number, before,
            "до подтверждения действует прежний ключ"
        );
        assert!(ann.outgoing("dm", "x".to_owned()).is_err());
        // Сообщение, зашифрованное новым ключом, читается и до подтверждения.
        other.directory_key("dm", &ann.public_key());
        let sealed = other
            .outgoing("dm", "с нового устройства".to_owned())
            .unwrap_or_default();
        let mut incoming = message("dm", "bob", 22, &sealed);
        ann.incoming(&mut incoming, true);
        assert_eq!(incoming.content.as_deref(), Some("с нового устройства"));
        ann.accept_new_key("dm");
        assert!(!ann.status("dm").key_changed);
        assert_ne!(ann.status("dm").safety_number, before);
        assert!(ann.outgoing("dm", "x".to_owned()).is_ok());
        // История, зашифрованная прежним ключом собеседника, читается и дальше.
        let old = bob
            .outgoing("dm", "из прошлого".to_owned())
            .unwrap_or_default();
        let mut from_history = message("dm", "bob", 12, &old);
        ann.incoming(&mut from_history, true);
        assert_eq!(from_history.content.as_deref(), Some("из прошлого"));
        // Старое объявление из истории новый ключ не отменяет.
        let stale = bs_e2ee::key_announcement(&bob.key.public());
        ann.incoming(&mut message("dm", "bob", 11, &stale), true);
        assert!(!ann.status("dm").key_changed);
    }

    #[test]
    fn groups_and_strangers_are_left_alone() {
        let (mut ann, mut bob) = (E2ee::load(None, "ann"), E2ee::load(None, "bob"));
        bob.set_enabled("group", true);
        let (_, announcement) = bob.outbox.remove(0);
        let mut in_group = message("group", "bob", 5, &announcement);
        ann.incoming(&mut in_group, false);
        assert_eq!(ann.status("group"), ChatStatus::default());
        assert!(ann.outbox.is_empty());
        let mut unreadable = message(
            "dm",
            "bob",
            6,
            "bs-e2ee:1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        ann.incoming(&mut unreadable, true);
        assert_eq!(
            unreadable.content.as_deref(),
            Some(tr("Не удалось расшифровать сообщение"))
        );
        let mut plain = message("dm", "bob", 7, "обычный текст");
        ann.incoming(&mut plain, true);
        assert_eq!(plain.content.as_deref(), Some("обычный текст"));
    }

    #[test]
    fn key_and_settings_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!(
            "bs-e2ee-test-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        let session = dir.join("session.json");
        let mut first = E2ee::load(Some(&session), "ann/../x");
        first.set_enabled("dm", true);
        let key = first.public_key();
        drop(first);
        let file = dir.join("e2ee-annx.json");
        assert!(file.is_file(), "имя файла не выходит из каталога");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&file)
                .map(|meta| meta.permissions().mode() & 0o777)
                .unwrap_or_default();
            assert_eq!(mode, 0o600);
        }
        let second = E2ee::load(Some(&session), "ann/../x");
        assert_eq!(second.public_key(), key);
        assert!(second.status("dm").enabled);
        assert!(second.outbox.is_empty(), "ключ второй раз не объявляется");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn key_moves_to_a_second_device() {
        let (mut ann, mut bob) = (E2ee::load(None, "ann"), E2ee::load(None, "bob"));
        ann.set_enabled("dm", true);
        deliver(&mut ann, &mut bob, 1);
        deliver(&mut bob, &mut ann, 2);
        let backup = ann.export_key("пароль для переноса").unwrap_or_default();
        assert!(ann.export_key("коротко").is_err());
        // Второе устройство: своё состояние, свой ключ, потом перенос.
        let mut second = E2ee::load(None, "ann");
        second.set_enabled("dm", true);
        second.outbox.clear();
        assert!(second.import_key(&backup, "не тот пароль").is_err());
        assert!(second.import_key(&backup, "пароль для переноса").is_ok());
        assert_eq!(second.public_key(), ann.public_key());
        assert_eq!(
            second.outbox.len(),
            1,
            "включённая переписка получает ключ заново"
        );
        // Собеседник уже знает этот ключ: смены нет.
        deliver(&mut second, &mut bob, 3);
        assert!(!bob.status("dm").key_changed);
    }

    #[test]
    fn missing_keyring_key_is_not_replaced() {
        let dir = std::env::temp_dir().join(format!(
            "bs-e2ee-kr-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        let session = dir.join("session.json");
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let file = dir.join("e2ee-ann.json");
        let body = br#"{"secretKey":"","keyInKeyring":true,"chats":{"dm":{"enabled":true}}}"#;
        std::fs::write(&file, body).unwrap_or_default();
        // В тестах хранилища нет: ключ из него не достать.
        let mut state = E2ee::load(Some(&session), "ann");
        assert!(state.locked());
        state.set_enabled("dm2", true);
        assert_eq!(
            std::fs::read(&file).unwrap_or_default(),
            body,
            "сохранённое не тронуто"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_nanos())
    }
}
