//! Клиент MTProto-прокси (`tg://proxy?server=…&port=…&secret=…`) для личных
//! мостов: например, tg-ws-proxy на роутере, который ведёт Telegram через
//! WebSocket, когда прямой доступ закрыт.
//!
//! grammers умеет только SOCKS5, поэтому мост поднимает у себя крошечный
//! SOCKS5 на 127.0.0.1: grammers подключается к нему «к дата-центру», а он
//! открывает соединение с MTProto-прокси — обфускация obfuscated2 с ключами
//! из секрета прокси и номером дата-центра в заголовке — и дальше гонит
//! транспорт Intermediate grammers, шифруя его в обе стороны.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock, PoisonError};

#[allow(deprecated, reason = "та же версия aes/ctr, что у grammers")]
use aes::cipher::{KeyIvInit, StreamCipher};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type Cipher = ctr::Ctr128BE<aes::Aes256>;

/// MTProto-прокси из настроек.
#[derive(Clone, PartialEq, Eq)]
pub struct MtProxy {
    /// Хост и порт прокси.
    pub server: String,
    pub port: u16,
    /// Секрет: 16 байт.
    secret: [u8; 16],
    /// Секрет с префиксом `dd`: прокси ждёт Padded Intermediate.
    padded: bool,
}

impl std::fmt::Debug for MtProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MtProxy")
            .field("server", &self.server)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl MtProxy {
    /// Разбирает `tg://proxy?…`, `https://t.me/proxy?…` или
    /// `mtproto://СЕКРЕТ@хост:порт`. Секреты fake-TLS (`ee…`) не подходят.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (server, port, secret) = if let Some(rest) = text.strip_prefix("mtproto://") {
            let (secret, address) = rest.split_once('@')?;
            let (server, port) = address.trim_end_matches('/').rsplit_once(':')?;
            (server.to_owned(), port.parse().ok()?, secret.to_owned())
        } else {
            let query = text
                .strip_prefix("tg://proxy?")
                .or_else(|| text.strip_prefix("https://t.me/proxy?"))
                .or_else(|| text.strip_prefix("http://t.me/proxy?"))?;
            let mut server = None;
            let mut port = None;
            let mut secret = None;
            for pair in query.split('&') {
                match pair.split_once('=') {
                    Some(("server", value)) => server = Some(value.to_owned()),
                    Some(("port", value)) => port = value.parse().ok(),
                    Some(("secret", value)) => secret = Some(value.to_owned()),
                    _ => {}
                }
            }
            (server?, port?, secret?)
        };
        let bytes = hex::decode(secret.trim()).ok()?;
        let (padded, key) = match bytes.as_slice() {
            [0xdd, key @ ..] if key.len() == 16 => (true, key),
            key if key.len() == 16 => (false, key),
            _ => return None,
        };
        let secret: [u8; 16] = key.try_into().ok()?;
        let server = server.trim_matches(['[', ']']).to_owned();
        (!server.is_empty() && port != 0).then_some(Self {
            server,
            port,
            secret,
            padded,
        })
    }
}

/// Адреса дата-центров Telegram → номер (из сессий grammers).
static DC_BY_ADDR: Mutex<Option<HashMap<IpAddr, i16>>> = Mutex::new(None);

/// Запоминает адреса дата-центра, чтобы знать, куда просить прокси.
pub fn remember_dc(id: i32, addresses: &[SocketAddr]) {
    let Ok(id) = i16::try_from(id) else {
        return;
    };
    let mut map = DC_BY_ADDR.lock().unwrap_or_else(PoisonError::into_inner);
    let map = map.get_or_insert_with(HashMap::new);
    for address in addresses {
        map.insert(address.ip(), id);
    }
}

fn dc_of(ip: IpAddr) -> i16 {
    DC_BY_ADDR
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|map| map.get(&ip).copied())
        .unwrap_or_else(|| {
            tracing::warn!(%ip, "неизвестный дата-центр Telegram, прошу DC2");
            2
        })
}

/// Адрес встроенного SOCKS5 (`socks5://127.0.0.1:порт`) для grammers;
/// поднимается при первом обращении.
pub async fn local_socks(proxy: &MtProxy) -> std::io::Result<String> {
    static STARTED: tokio::sync::OnceCell<SocketAddr> = tokio::sync::OnceCell::const_new();
    let proxy = proxy.clone();
    let address = STARTED
        .get_or_try_init(|| async move {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
            let address = listener.local_addr()?;
            tokio::spawn(async move {
                loop {
                    let Ok((client, _)) = listener.accept().await else {
                        continue;
                    };
                    let proxy = proxy.clone();
                    tokio::spawn(async move {
                        if let Err(err) = serve(client, &proxy).await {
                            tracing::debug!(%err, "соединение через MTProto-прокси закрыто");
                        }
                    });
                }
            });
            Ok::<_, std::io::Error>(address)
        })
        .await?;
    Ok(format!("socks5://{address}"))
}

/// Один клиент grammers: SOCKS5 → MTProto-прокси.
async fn serve(mut client: TcpStream, proxy: &MtProxy) -> std::io::Result<()> {
    let target = socks_handshake(&mut client).await?;
    // grammers начинает транспорт Intermediate с метки 0xeeeeeeee: в
    // обфускации она едет внутри заголовка.
    let mut tag = [0_u8; 4];
    client.read_exact(&mut tag).await?;
    if tag != [0xee; 4] {
        return Err(std::io::Error::other("ждали транспорт Intermediate"));
    }
    let mut upstream = TcpStream::connect((proxy.server.as_str(), proxy.port)).await?;
    upstream.set_nodelay(true)?;
    let (header, mut encrypt, mut decrypt) = handshake(proxy, dc_of(target.ip()));
    upstream.write_all(&header).await?;
    let (mut client_read, mut client_write) = client.into_split();
    let (mut proxy_read, mut proxy_write) = upstream.into_split();
    let outgoing = async move {
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let read = client_read.read(&mut buffer).await?;
            if read == 0 {
                return Ok::<_, std::io::Error>(());
            }
            encrypt.apply_keystream(&mut buffer[..read]);
            proxy_write.write_all(&buffer[..read]).await?;
        }
    };
    let padded = proxy.padded;
    let incoming = async move {
        let mut buffer = vec![0_u8; 16 * 1024];
        let mut pending = Vec::new();
        loop {
            let read = proxy_read.read(&mut buffer).await?;
            if read == 0 {
                return Ok::<_, std::io::Error>(());
            }
            decrypt.apply_keystream(&mut buffer[..read]);
            if padded {
                pending.extend_from_slice(&buffer[..read]);
                let frames = unpad(&mut pending);
                if !frames.is_empty() {
                    client_write.write_all(&frames).await?;
                }
            } else {
                client_write.write_all(&buffer[..read]).await?;
            }
        }
    };
    tokio::select! {
        result = outgoing => result,
        result = incoming => result,
    }
}

/// Padded Intermediate → Intermediate: из готовых кадров срезается
/// случайное дополнение (длина кадра не кратна 4), которого grammers не ждёт.
/// Незаконченный кадр остаётся в `pending`.
fn unpad(pending: &mut Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while pending.len() >= at + 4 {
        let head = [
            pending[at],
            pending[at + 1],
            pending[at + 2],
            pending[at + 3],
        ];
        let len = u32::from_le_bytes(head);
        if len & 0x8000_0000 != 0 {
            // Быстрое подтверждение: четыре байта без тела.
            out.extend_from_slice(&head);
            at += 4;
            continue;
        }
        let Ok(len) = usize::try_from(len) else {
            break;
        };
        if pending.len() < at + 4 + len {
            break;
        }
        let body = &pending[at + 4..at + 4 + len - len % 4];
        let body_len = u32::try_from(body.len()).unwrap_or_default();
        out.extend_from_slice(&body_len.to_le_bytes());
        out.extend_from_slice(body);
        at += 4 + len;
    }
    pending.drain(..at);
    out
}

/// SOCKS5 без пароля, только CONNECT; возвращает, куда просились.
async fn socks_handshake(client: &mut TcpStream) -> std::io::Result<SocketAddr> {
    let mut head = [0_u8; 2];
    client.read_exact(&mut head).await?;
    let mut methods = vec![0_u8; usize::from(head[1])];
    client.read_exact(&mut methods).await?;
    if head[0] != 5 || !methods.contains(&0) {
        client.write_all(&[5, 0xff]).await?;
        return Err(std::io::Error::other("не SOCKS5 без пароля"));
    }
    client.write_all(&[5, 0]).await?;
    let mut request = [0_u8; 4];
    client.read_exact(&mut request).await?;
    if request[1] != 1 {
        client.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Err(std::io::Error::other("не CONNECT"));
    }
    let ip: IpAddr = match request[3] {
        1 => {
            let mut v4 = [0_u8; 4];
            client.read_exact(&mut v4).await?;
            v4.into()
        }
        4 => {
            let mut v6 = [0_u8; 16];
            client.read_exact(&mut v6).await?;
            v6.into()
        }
        _ => {
            client.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Err(std::io::Error::other("адрес не IP"));
        }
    };
    let mut port = [0_u8; 2];
    client.read_exact(&mut port).await?;
    client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    Ok(SocketAddr::new(ip, u16::from_be_bytes(port)))
}

/// Заголовок obfuscated2 для прокси с секретом и шифры обеих сторон.
fn handshake(proxy: &MtProxy, dc: i16) -> ([u8; 64], Cipher, Cipher) {
    let mut init = [0_u8; 64];
    loop {
        rand::fill(&mut init[..]);
        let first = [init[0], init[1], init[2], init[3]];
        let forbidden = init[0] == 0xef
            || init[4..8] == [0; 4]
            || [
                *b"HEAD",
                *b"POST",
                *b"GET ",
                *b"OPTI",
                [0x16, 0x03, 0x01, 0x02],
                [0xdd; 4],
                [0xee; 4],
            ]
            .contains(&first);
        if !forbidden {
            break;
        }
    }
    let tag = if proxy.padded { [0xdd; 4] } else { [0xee; 4] };
    init[56..60].copy_from_slice(&tag);
    init[60..62].copy_from_slice(&dc.to_le_bytes());
    let key = |part: &[u8]| -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(part);
        hash.update(proxy.secret);
        hash.finalize().into()
    };
    let mut reversed = [0_u8; 48];
    reversed.copy_from_slice(&init[8..56]);
    reversed.reverse();
    let mut iv_out = [0_u8; 16];
    iv_out.copy_from_slice(&init[40..56]);
    let mut iv_in = [0_u8; 16];
    iv_in.copy_from_slice(&reversed[32..48]);
    #[allow(deprecated, reason = "та же версия aes/ctr, что у grammers")]
    let mut encrypt = Cipher::new(&key(&init[8..40]).into(), &iv_out.into());
    #[allow(deprecated, reason = "та же версия aes/ctr, что у grammers")]
    let decrypt = Cipher::new(&key(&reversed[..32]).into(), &iv_in.into());
    let mut encrypted = init;
    encrypt.apply_keystream(&mut encrypted);
    init[56..64].copy_from_slice(&encrypted[56..64]);
    (init, encrypt, decrypt)
}

/// Свои адреса дата-центров по умолчанию (на случай пустой сессии).
pub fn remember_defaults(options: &[grammers_session::types::DcOption]) {
    for option in options {
        remember_dc(
            option.id,
            &[SocketAddr::V4(option.ipv4), SocketAddr::V6(option.ipv6)],
        );
    }
}

/// Прокси из `TELEGRAM_PROXY`, если там MTProto-ссылка.
static CONFIGURED: OnceLock<Option<MtProxy>> = OnceLock::new();

/// Разобранный MTProto-прокси настроек (кэшируется).
pub fn configured(raw: Option<&str>) -> Option<&'static MtProxy> {
    CONFIGURED
        .get_or_init(|| raw.and_then(MtProxy::parse))
        .as_ref()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::{MtProxy, handshake};

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn links_are_parsed() {
        let tg = MtProxy::parse(&format!(
            "tg://proxy?server=192.168.1.1&port=1443&secret=dd{SECRET}"
        ))
        .expect("tg://");
        assert_eq!(
            (tg.server.as_str(), tg.port, tg.padded),
            ("192.168.1.1", 1443, true)
        );
        let link = MtProxy::parse(&format!(
            "https://t.me/proxy?server=router&port=443&secret={SECRET}"
        ))
        .expect("t.me");
        assert!(!link.padded);
        let short =
            MtProxy::parse(&format!("mtproto://{SECRET}@10.0.0.2:1443")).expect("mtproto://");
        assert_eq!(short.server, "10.0.0.2");
        // fake-TLS и мусор не берём.
        assert!(
            MtProxy::parse(&format!(
                "tg://proxy?server=a&port=1&secret=ee{SECRET}6578616d706c65"
            ))
            .is_none()
        );
        assert!(MtProxy::parse("socks5://127.0.0.1:1080").is_none());
        assert!(format!("{tg:?}").contains("192.168.1.1") && !format!("{tg:?}").contains(SECRET));
    }

    /// Прокси расшифровывает заголовок своим ключом из секрета: проверяем,
    /// что метка протокола и номер дата-центра доходят.
    #[test]
    fn header_carries_tag_and_dc() {
        use aes::cipher::StreamCipher;
        use sha2::{Digest, Sha256};
        let proxy = MtProxy::parse(&format!("mtproto://dd{SECRET}@h:1")).expect("прокси");
        let (header, _, _) = handshake(&proxy, 4);
        let mut hash = Sha256::new();
        hash.update(&header[8..40]);
        hash.update(proxy.secret);
        let key: [u8; 32] = hash.finalize().into();
        #[allow(deprecated, reason = "та же версия aes/ctr, что у grammers")]
        let mut iv = [0_u8; 16];
        iv.copy_from_slice(&header[40..56]);
        let mut cipher = <super::Cipher as aes::cipher::KeyIvInit>::new(&key.into(), &iv.into());
        let mut plain = header;
        cipher.apply_keystream(&mut plain);
        assert_eq!(&plain[56..60], &[0xdd; 4]);
        assert_eq!(i16::from_le_bytes([plain[60], plain[61]]), 4);
    }

    #[test]
    fn padding_is_cut_from_frames() {
        let mut pending = Vec::new();
        pending.extend_from_slice(&6_u32.to_le_bytes());
        pending.extend_from_slice(b"abcdXY");
        pending.extend_from_slice(&8_u32.to_le_bytes());
        pending.extend_from_slice(b"efgh");
        let out = super::unpad(&mut pending);
        let mut want = 4_u32.to_le_bytes().to_vec();
        want.extend_from_slice(b"abcd");
        assert_eq!(out, want);
        assert_eq!(pending.len(), 8, "второй кадр ещё не дошёл");
        pending.extend_from_slice(b"ijkl");
        let out = super::unpad(&mut pending);
        assert_eq!(&out[4..], b"efghijkl");
        assert!(pending.is_empty());
    }

    /// Весь путь: SOCKS5 → наш мост → «прокси», который расшифровывает
    /// секретом заголовок и данные и отвечает своим шифром.
    #[tokio::test]
    async fn bytes_travel_through_the_proxy() {
        use aes::cipher::{KeyIvInit, StreamCipher};
        use sha2::{Digest, Sha256};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let fake = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("порт");
        let port = fake.local_addr().expect("адрес").port();
        let proxy =
            MtProxy::parse(&format!("mtproto://dd{SECRET}@127.0.0.1:{port}")).expect("прокси");
        super::remember_dc(4, &["149.154.167.91:443".parse().expect("адрес")]);
        let secret = proxy.secret;
        let server = tokio::spawn(async move {
            let (mut stream, _) = fake.accept().await.expect("клиент");
            let mut header = [0_u8; 64];
            stream.read_exact(&mut header).await.expect("заголовок");
            let derive = |part: &[u8]| -> [u8; 32] {
                let mut hash = Sha256::new();
                hash.update(part);
                hash.update(secret);
                hash.finalize().into()
            };
            let mut iv = [0_u8; 16];
            iv.copy_from_slice(&header[40..56]);
            let mut rx = super::Cipher::new(&derive(&header[8..40]).into(), &iv.into());
            let mut reversed = [0_u8; 48];
            reversed.copy_from_slice(&header[8..56]);
            reversed.reverse();
            let mut iv = [0_u8; 16];
            iv.copy_from_slice(&reversed[32..48]);
            let mut tx = super::Cipher::new(&derive(&reversed[..32]).into(), &iv.into());
            let mut plain = header;
            rx.apply_keystream(&mut plain);
            assert_eq!(&plain[56..60], &[0xdd; 4]);
            assert_eq!(i16::from_le_bytes([plain[60], plain[61]]), 4);
            let mut data = [0_u8; 5];
            stream.read_exact(&mut data).await.expect("данные");
            rx.apply_keystream(&mut data);
            assert_eq!(&data, b"hello");
            // Кадр Padded Intermediate: 4 байта данных и 2 байта дополнения.
            let mut reply = 6_u32.to_le_bytes().to_vec();
            reply.extend_from_slice(b"wrldXY");
            tx.apply_keystream(&mut reply);
            stream.write_all(&reply).await.expect("ответ");
        });
        let socks = super::local_socks(&proxy).await.expect("socks");
        let address = socks.trim_start_matches("socks5://");
        let mut client = tokio::net::TcpStream::connect(address)
            .await
            .expect("socks");
        client.write_all(&[5, 1, 0]).await.expect("приветствие");
        let mut answer = [0_u8; 2];
        client.read_exact(&mut answer).await.expect("метод");
        assert_eq!(answer, [5, 0]);
        client
            .write_all(&[5, 1, 0, 1, 149, 154, 167, 91, 1, 187])
            .await
            .expect("connect");
        let mut reply = [0_u8; 10];
        client.read_exact(&mut reply).await.expect("ответ socks");
        assert_eq!(reply[1], 0);
        client.write_all(&[0xee; 4]).await.expect("метка");
        client.write_all(b"hello").await.expect("данные");
        let mut back = [0_u8; 8];
        client.read_exact(&mut back).await.expect("ответ");
        assert_eq!(&back[..4], &4_u32.to_le_bytes(), "дополнение срезано");
        assert_eq!(&back[4..], b"wrld");
        server.await.expect("прокси");
    }
}
