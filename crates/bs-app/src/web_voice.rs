//! Голос в веб-версии: свой маленький клиент LiveKit поверх WebRTC браузера.
//!
//! Сигнализация — протокол LiveKit (protobuf по WebSocket, `livekit-protocol`),
//! звук — `RTCPeerConnection` браузера: одно соединение принимает звук
//! комнаты (его предлагает сервер), второе отдаёт свой микрофон. Слушаем
//! только микрофоны собеседников; звук трансляций и видео не запрашиваем.
//! Интерфейс тот же, что у настольного движка (`MediaEngine`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use livekit_protocol::{
    AddTrackRequest, LeaveRequest, MuteTrackRequest, ParticipantInfo, SessionDescription,
    SignalRequest, SignalResponse, TrickleRequest, UpdateSubscription, signal_request,
    signal_response,
};
use prost::Message as _;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    HtmlAudioElement, MediaStream, MediaStreamConstraints, MediaStreamTrack, MessageEvent,
    RtcConfiguration, RtcIceCandidateInit, RtcIceServer, RtcPeerConnection,
    RtcPeerConnectionIceEvent, RtcRtpTransceiverDirection, RtcRtpTransceiverInit, RtcSdpType,
    RtcSessionDescriptionInit, RtcTrackEvent, WebSocket,
};

use super::{ConnectRequest, Device, DeviceKind, MediaStatus, Speaking, VoicePrefs};
use crate::i18n::tr;

/// `TrackType::Audio`.
const AUDIO: i32 = 0;
/// `TrackSource::Microphone`.
const MICROPHONE: i32 = 2;
/// `SignalTarget::Publisher` и `SignalTarget::Subscriber`.
const PUBLISHER: i32 = 0;
const SUBSCRIBER: i32 = 1;
/// `ParticipantInfo.State::Disconnected`.
const DISCONNECTED: i32 = 3;

/// Комната: всё, что живёт, пока мы в звонке.
#[derive(Default)]
struct Room {
    socket: Option<WebSocket>,
    publisher: Option<RtcPeerConnection>,
    subscriber: Option<RtcPeerConnection>,
    mic: Option<MediaStream>,
    /// Наша дорожка микрофона на сервере (sid), когда опубликована.
    mic_sid: Option<String>,
    me: Option<(String, String)>,
    /// Участники: sid → (id пользователя, имя).
    people: HashMap<String, (String, String)>,
    /// Дорожки микрофонов: sid дорожки → id пользователя.
    mics: HashMap<String, String>,
    subscribed: HashSet<String>,
    /// Звук собеседников: id пользователя → элементы `<audio>`.
    players: HashMap<String, Vec<HtmlAudioElement>>,
    volumes: HashMap<String, u16>,
    speaking: Speaking,
    muted: bool,
    deafened: bool,
    status: MediaStatus,
    /// Обработчики событий браузера живут, пока живёт комната.
    handlers: Vec<Closure<dyn FnMut(JsValue)>>,
    /// Номер подключения: ответы старого подключения не трогают новое.
    generation: u64,
}

type Shared = Rc<RefCell<Room>>;

/// Голос через WebRTC браузера.
pub(crate) struct MediaEngine {
    room: Shared,
}

impl std::fmt::Debug for MediaEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaEngine")
            .field("status", &self.room.borrow().status)
            .finish_non_exhaustive()
    }
}

impl Default for MediaEngine {
    fn default() -> Self {
        Self {
            room: Rc::new(RefCell::new(Room::default())),
        }
    }
}

/// Id и имя участника: identity в комнате — `id:имя`, а имя в событии
/// бывает пустым (у пользователей других инстансов).
fn member(identity: &str, name: &str) -> (String, String) {
    let (id, rest) = identity.split_once(':').unwrap_or((identity, ""));
    let name = if !name.trim().is_empty() {
        name
    } else if rest.trim().is_empty() {
        id
    } else {
        rest.trim()
    };
    (id.to_owned(), name.to_owned())
}

fn send(room: &Room, message: signal_request::Message) {
    let Some(socket) = &room.socket else {
        return;
    };
    let bytes = SignalRequest {
        message: Some(message),
    }
    .encode_to_vec();
    if let Err(err) = socket.send_with_u8_array(&bytes) {
        tracing::debug!(?err, "сигнал LiveKit не отправлен");
    }
}

fn js_error(err: &JsValue) -> String {
    err.as_string()
        .or_else(|| {
            js_sys::Reflect::get(err, &"message".into())
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| format!("{err:?}"))
}

/// Адрес сигнализации LiveKit: `wss://…/rtc?access_token=…`.
fn signal_url(url: &str, token: &str) -> String {
    let base = url
        .trim()
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    format!("{base}/rtc?access_token={token}&auto_subscribe=0&sdk=rust&protocol=15&version=1.0.0")
}

fn peer(ice: &[livekit_protocol::IceServer]) -> Result<RtcPeerConnection, JsValue> {
    let servers = js_sys::Array::new();
    for server in ice {
        let entry = RtcIceServer::new();
        let urls: js_sys::Array = server.urls.iter().map(|u| JsValue::from_str(u)).collect();
        entry.set_urls(&urls);
        if !server.username.is_empty() {
            entry.set_username(&server.username);
            entry.set_credential(&server.credential);
        }
        servers.push(&entry);
    }
    let config = RtcConfiguration::new();
    config.set_ice_servers(&servers);
    RtcPeerConnection::new_with_configuration(&config)
}

/// Свои кандидаты ICE — серверу.
fn forward_candidates(shared: &Shared, pc: &RtcPeerConnection, target: i32) {
    let weak = Rc::downgrade(shared);
    let handler = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let Some(room) = weak.upgrade() else {
            return;
        };
        let Some(candidate) = event
            .dyn_into::<RtcPeerConnectionIceEvent>()
            .ok()
            .and_then(|event| event.candidate())
        else {
            return;
        };
        let json = js_sys::JSON::stringify(&candidate.to_json())
            .ok()
            .and_then(|s| s.as_string())
            .unwrap_or_default();
        send(
            &room.borrow(),
            signal_request::Message::Trickle(TrickleRequest {
                candidate_init: json,
                target,
                ..Default::default()
            }),
        );
    });
    pc.set_onicecandidate(Some(handler.as_ref().unchecked_ref()));
    shared.borrow_mut().handlers.push(handler);
}

/// Чужой кандидат ICE — в соединение.
async fn add_candidate(pc: &RtcPeerConnection, json: &str) -> Result<(), JsValue> {
    let value: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let init = RtcIceCandidateInit::new(value["candidate"].as_str().unwrap_or_default());
    init.set_sdp_mid(value["sdpMid"].as_str());
    init.set_sdp_m_line_index(
        value["sdpMLineIndex"]
            .as_u64()
            .and_then(|i| u16::try_from(i).ok()),
    );
    JsFuture::from(pc.add_ice_candidate_with_opt_rtc_ice_candidate_init(Some(&init))).await?;
    Ok(())
}

fn sdp_of(value: &JsValue) -> String {
    js_sys::Reflect::get(value, &"sdp".into())
        .ok()
        .and_then(|sdp| sdp.as_string())
        .unwrap_or_default()
}

/// Звук собеседников: принимаем дорожку и играем её `<audio>`.
fn play_tracks(shared: &Shared, pc: &RtcPeerConnection) {
    let weak = Rc::downgrade(shared);
    let handler = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let Some(shared) = weak.upgrade() else {
            return;
        };
        let Ok(event) = event.dyn_into::<RtcTrackEvent>() else {
            return;
        };
        if event.track().kind() != "audio" {
            return;
        }
        let Some(stream) = event.streams().get(0).dyn_into::<MediaStream>().ok() else {
            return;
        };
        // Поток LiveKit называется «PA_участника|TR_дорожки».
        let id = stream.id();
        let track_sid = id.split('|').nth(1).unwrap_or_default().to_owned();
        let mut room = shared.borrow_mut();
        let Some(identity) = room.mics.get(&track_sid).cloned() else {
            return;
        };
        let Ok(player) = HtmlAudioElement::new() else {
            return;
        };
        player.set_autoplay(true);
        player.set_src_object(Some(&stream));
        player.set_muted(room.deafened);
        let volume = room.volumes.get(&identity).copied().unwrap_or(100);
        player.set_volume((f64::from(volume) / 100.0).clamp(0.0, 1.0));
        if let Ok(started) = player.play() {
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(err) = JsFuture::from(started).await {
                    tracing::debug!(err = %js_error(&err), "звук не заиграл");
                }
            });
        }
        room.players.entry(identity).or_default().push(player);
    });
    pc.set_ontrack(Some(handler.as_ref().unchecked_ref()));
    shared.borrow_mut().handlers.push(handler);
}

/// Подписка на микрофоны новых участников.
fn learn_people(room: &mut Room, people: &[ParticipantInfo]) {
    let mut wanted = Vec::new();
    for person in people {
        let mine = room.me.as_ref().is_some_and(|(_, sid)| *sid == person.sid);
        let (id, name) = member(&person.identity, &person.name);
        if person.state == DISCONNECTED {
            room.people.remove(&person.sid);
            room.mics.retain(|_, user| *user != id);
            room.players.remove(&id);
            room.speaking.remove(&id);
            continue;
        }
        if mine {
            continue;
        }
        room.people.insert(person.sid.clone(), (id.clone(), name));
        for track in &person.tracks {
            if track.r#type == AUDIO && track.source == MICROPHONE {
                room.mics.insert(track.sid.clone(), id.clone());
                if room.subscribed.insert(track.sid.clone()) {
                    wanted.push(track.sid.clone());
                }
            }
        }
    }
    if !wanted.is_empty() {
        send(
            room,
            signal_request::Message::Subscription(UpdateSubscription {
                track_sids: wanted,
                subscribe: true,
                ..Default::default()
            }),
        );
    }
}

/// Предлагает серверу своё соединение с микрофоном.
async fn negotiate_publisher(shared: &Shared) -> Result<(), JsValue> {
    let Some(pc) = shared.borrow().publisher.clone() else {
        return Ok(());
    };
    let offer = JsFuture::from(pc.create_offer()).await?;
    let init = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
    let sdp = sdp_of(&offer);
    init.set_sdp(&sdp);
    JsFuture::from(pc.set_local_description(&init)).await?;
    send(
        &shared.borrow(),
        signal_request::Message::Offer(SessionDescription {
            r#type: "offer".to_owned(),
            sdp,
            ..Default::default()
        }),
    );
    Ok(())
}

/// Включает микрофон и просит сервер принять дорожку.
async fn open_mic(shared: &Shared) -> Result<(), String> {
    let devices = web_sys::window()
        .ok_or_else(|| tr("нет окна браузера").to_owned())?
        .navigator()
        .media_devices()
        .map_err(|err| js_error(&err))?;
    let constraints = MediaStreamConstraints::new();
    let audio = js_sys::Object::new();
    for key in ["echoCancellation", "noiseSuppression", "autoGainControl"] {
        let _ = js_sys::Reflect::set(&audio, &key.into(), &JsValue::TRUE);
    }
    constraints.set_audio(&audio);
    let stream: MediaStream = JsFuture::from(
        devices
            .get_user_media_with_constraints(&constraints)
            .map_err(|err| js_error(&err))?,
    )
    .await
    .map_err(|err| js_error(&err))?
    .dyn_into()
    .map_err(|err| js_error(&err))?;
    let Some(track) = stream
        .get_audio_tracks()
        .get(0)
        .dyn_into::<MediaStreamTrack>()
        .ok()
    else {
        return Err(tr("браузер не дал микрофон").to_owned());
    };
    let mut room = shared.borrow_mut();
    track.set_enabled(!room.muted && !room.deafened);
    send(
        &room,
        signal_request::Message::AddTrack(AddTrackRequest {
            cid: track.id(),
            name: "microphone".to_owned(),
            r#type: AUDIO,
            source: MICROPHONE,
            muted: room.muted || room.deafened,
            ..Default::default()
        }),
    );
    if let Some(pc) = &room.publisher {
        let init = RtcRtpTransceiverInit::new();
        init.set_direction(RtcRtpTransceiverDirection::Sendonly);
        init.set_streams(&js_sys::Array::of1(&stream));
        pc.add_transceiver_with_media_stream_track_and_init(&track, &init);
    }
    room.mic = Some(stream);
    Ok(())
}

/// Обрабатывает сигналы сервера по одному, по порядку.
#[allow(
    clippy::too_many_lines,
    reason = "разбор всех нужных сигналов в одном месте"
)]
async fn handle(shared: &Shared, message: signal_response::Message) -> Result<(), JsValue> {
    match message {
        signal_response::Message::Join(join) => {
            let publisher = peer(&join.ice_servers)?;
            let subscriber = peer(&join.ice_servers)?;
            forward_candidates(shared, &publisher, PUBLISHER);
            forward_candidates(shared, &subscriber, SUBSCRIBER);
            play_tracks(shared, &subscriber);
            {
                let mut room = shared.borrow_mut();
                room.publisher = Some(publisher);
                room.subscriber = Some(subscriber);
                room.me = join
                    .participant
                    .as_ref()
                    .map(|me| (member(&me.identity, &me.name).0, me.sid.clone()));
                learn_people(&mut room, &join.other_participants);
            }
            start_ping(shared, join.ping_interval);
            match open_mic(shared).await {
                Ok(()) => shared.borrow_mut().status = MediaStatus::Live,
                Err(err) => {
                    shared.borrow_mut().status = MediaStatus::ListenOnly(err);
                }
            }
        }
        signal_response::Message::Offer(offer) => {
            let Some(pc) = shared.borrow().subscriber.clone() else {
                return Ok(());
            };
            let remote = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
            remote.set_sdp(&offer.sdp);
            JsFuture::from(pc.set_remote_description(&remote)).await?;
            let answer = JsFuture::from(pc.create_answer()).await?;
            let sdp = sdp_of(&answer);
            let local = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
            local.set_sdp(&sdp);
            JsFuture::from(pc.set_local_description(&local)).await?;
            send(
                &shared.borrow(),
                signal_request::Message::Answer(SessionDescription {
                    r#type: "answer".to_owned(),
                    sdp,
                    id: offer.id,
                    ..Default::default()
                }),
            );
        }
        signal_response::Message::Answer(answer) => {
            let Some(pc) = shared.borrow().publisher.clone() else {
                return Ok(());
            };
            let remote = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
            remote.set_sdp(&answer.sdp);
            JsFuture::from(pc.set_remote_description(&remote)).await?;
        }
        signal_response::Message::Trickle(trickle) => {
            let pc = {
                let room = shared.borrow();
                if trickle.target == PUBLISHER {
                    room.publisher.clone()
                } else {
                    room.subscriber.clone()
                }
            };
            if let Some(pc) = pc {
                add_candidate(&pc, &trickle.candidate_init).await?;
            }
        }
        signal_response::Message::TrackPublished(published) => {
            let sid = published.track.map(|track| track.sid);
            shared.borrow_mut().mic_sid = sid;
            negotiate_publisher(shared).await?;
        }
        signal_response::Message::Update(update) => {
            learn_people(&mut shared.borrow_mut(), &update.participants);
        }
        signal_response::Message::SpeakersChanged(changed) => {
            let mut room = shared.borrow_mut();
            for speaker in changed.speakers {
                let identity = if room.me.as_ref().is_some_and(|(_, sid)| *sid == speaker.sid) {
                    room.me.as_ref().map(|(identity, _)| identity.clone())
                } else {
                    room.people
                        .get(&speaker.sid)
                        .map(|(identity, _)| identity.clone())
                };
                if let Some(identity) = identity {
                    if speaker.active {
                        room.speaking.insert(identity);
                    } else {
                        room.speaking.remove(&identity);
                    }
                }
            }
        }
        signal_response::Message::Leave(_) => {
            let mut room = shared.borrow_mut();
            room.status = MediaStatus::Failed(tr("Сервер голоса завершил соединение").to_owned());
            close(&mut room);
        }
        _ => {}
    }
    Ok(())
}

/// Пинг сигнализации: без него сервер разорвёт соединение.
fn start_ping(shared: &Shared, interval: i32) {
    let seconds = u64::try_from(interval)
        .ok()
        .filter(|s| *s > 0)
        .unwrap_or(10);
    let weak = Rc::downgrade(shared);
    let generation = shared.borrow().generation;
    wasm_bindgen_futures::spawn_local(async move {
        loop {
            bs_rt::time::sleep(std::time::Duration::from_secs(seconds)).await;
            let Some(shared) = weak.upgrade() else {
                return;
            };
            let room = shared.borrow();
            if room.generation != generation || room.socket.is_none() {
                return;
            }
            #[allow(clippy::cast_possible_truncation, reason = "миллисекунды эпохи")]
            let now = js_sys::Date::now() as i64;
            send(&room, signal_request::Message::Ping(now));
        }
    });
}

/// Закрывает всё, что открыто в комнате.
fn close(room: &mut Room) {
    if let Some(socket) = room.socket.take() {
        socket.set_onmessage(None);
        socket.set_onclose(None);
        let _ = socket.close();
    }
    for pc in [room.publisher.take(), room.subscriber.take()]
        .into_iter()
        .flatten()
    {
        pc.set_onicecandidate(None);
        pc.set_ontrack(None);
        pc.close();
    }
    if let Some(mic) = room.mic.take() {
        for track in mic.get_tracks().iter() {
            if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                track.stop();
            }
        }
    }
    for player in room.players.drain().flat_map(|(_, list)| list) {
        player.set_src_object(None);
    }
    room.handlers.clear();
    room.people.clear();
    room.mics.clear();
    room.subscribed.clear();
    room.speaking.clear();
    room.mic_sid = None;
    room.me = None;
}

#[allow(
    clippy::unused_self,
    clippy::needless_pass_by_ref_mut,
    clippy::unnecessary_wraps,
    reason = "тот же интерфейс, что у настольного движка"
)]
impl MediaEngine {
    pub(crate) fn new(_handle: crate::rt::Handle) -> Self {
        Self::default()
    }

    pub(crate) const fn available() -> bool {
        true
    }

    /// Устройства выбирает браузер (его настройки сайта).
    pub(crate) fn devices(&mut self, _kind: DeviceKind, _prefs: &VoicePrefs) -> Vec<Device> {
        Vec::new()
    }

    pub(crate) fn apply_prefs(&mut self, _prefs: &VoicePrefs) {}

    pub(crate) fn connect(&mut self, request: ConnectRequest, _prefs: &VoicePrefs) {
        let mut room = self.room.borrow_mut();
        close(&mut room);
        room.generation += 1;
        room.muted = request.muted;
        room.deafened = request.deafened;
        room.status = MediaStatus::Connecting;
        let socket = match WebSocket::new(&signal_url(&request.url, &request.token)) {
            Ok(socket) => socket,
            Err(err) => {
                room.status = MediaStatus::Failed(js_error(&err));
                return;
            }
        };
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let (tx, mut rx) = bs_rt::sync::mpsc::unbounded_channel::<signal_response::Message>();
        let on_message = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
            let Some(data) = event
                .dyn_into::<MessageEvent>()
                .ok()
                .map(|event| event.data())
            else {
                return;
            };
            let bytes = js_sys::Uint8Array::new(&data).to_vec();
            if let Ok(SignalResponse {
                message: Some(message),
            }) = SignalResponse::decode(bytes.as_slice())
            {
                let _ = tx.send(message);
            }
        });
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        let weak = Rc::downgrade(&self.room);
        let generation = room.generation;
        let on_close = Closure::<dyn FnMut(JsValue)>::new(move |_| {
            if let Some(shared) = weak.upgrade() {
                let mut room = shared.borrow_mut();
                if room.generation == generation && room.socket.is_some() {
                    room.status = MediaStatus::Failed(
                        tr("Соединение с сервером голоса оборвалось").to_owned(),
                    );
                    close(&mut room);
                }
            }
        });
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        room.handlers.push(on_message);
        room.handlers.push(on_close);
        room.socket = Some(socket);
        drop(room);
        let shared = Rc::clone(&self.room);
        wasm_bindgen_futures::spawn_local(async move {
            while let Some(message) = rx.recv().await {
                if shared.borrow().generation != generation {
                    return;
                }
                if let Err(err) = handle(&shared, message).await {
                    tracing::warn!(err = %js_error(&err), "сигнал LiveKit не обработан");
                }
            }
        });
    }

    pub(crate) fn set_flags(&self, muted: bool, deafened: bool) {
        let mut room = self.room.borrow_mut();
        room.muted = muted;
        room.deafened = deafened;
        let silent = muted || deafened;
        if let Some(mic) = &room.mic {
            for track in mic.get_audio_tracks().iter() {
                if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                    track.set_enabled(!silent);
                }
            }
        }
        for player in room.players.values().flatten() {
            player.set_muted(deafened);
        }
        if let Some(sid) = room.mic_sid.clone() {
            send(
                &room,
                signal_request::Message::Mute(MuteTrackRequest { sid, muted: silent }),
            );
        }
    }

    pub(crate) fn set_mic_bitrate(&self, _kbps: u32) {}

    pub(crate) fn disconnect(&mut self) {
        let mut room = self.room.borrow_mut();
        send(
            &room,
            signal_request::Message::Leave(LeaveRequest {
                can_reconnect: false,
                ..Default::default()
            }),
        );
        close(&mut room);
        room.generation += 1;
        room.status = MediaStatus::Idle;
    }

    pub(crate) fn status(&self) -> MediaStatus {
        self.room.borrow().status.clone()
    }

    pub(crate) fn speaking(&self) -> Speaking {
        self.room.borrow().speaking.clone()
    }

    pub(crate) fn request_net_stats(&self) {}

    pub(crate) fn members(&self) -> Vec<(String, String)> {
        self.room.borrow().people.values().cloned().collect()
    }

    pub(crate) fn me_id(&self) -> Option<String> {
        self.room
            .borrow()
            .me
            .as_ref()
            .map(|(identity, _)| identity.clone())
    }

    pub(crate) fn net_stats(&self) -> Option<super::NetStats> {
        None
    }

    pub(crate) fn start_mic_test(&mut self, _prefs: &VoicePrefs) -> Result<(), String> {
        Err(tr("проверка микрофона есть только в настольной версии").to_owned())
    }

    pub(crate) fn stop_mic_test(&mut self) {}

    pub(crate) fn mic_level(&self) -> Option<f32> {
        None
    }

    pub(crate) fn set_stream_volume(&self, _user_id: &str, _percent: u16) {}

    pub(crate) fn set_voice_volume(&self, user_id: &str, percent: u16) {
        let mut room = self.room.borrow_mut();
        room.volumes.insert(user_id.to_owned(), percent);
        for player in room.players.get(user_id).into_iter().flatten() {
            player.set_volume((f64::from(percent) / 100.0).clamp(0.0, 1.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::signal_url;

    #[test]
    fn member_takes_the_name_from_identity() {
        assert_eq!(
            super::member("1:anna", ""),
            ("1".to_owned(), "anna".to_owned())
        );
        assert_eq!(
            super::member("1:anna", "Anna"),
            ("1".to_owned(), "Anna".to_owned())
        );
        assert_eq!(super::member("1", ""), ("1".to_owned(), "1".to_owned()));
    }

    #[test]
    fn signal_address() {
        assert!(
            signal_url("https://lk.example.org/", "T")
                .starts_with("wss://lk.example.org/rtc?access_token=T&")
        );
        assert!(signal_url("ws://127.0.0.1:7880", "T").starts_with("ws://127.0.0.1:7880/rtc?"));
    }
}
