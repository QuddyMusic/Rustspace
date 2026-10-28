//! Доменный координатор голосового протокола.
//!
//! Координатор не выполняет сетевые операции: он проверяет входящие события,
//! хранит текущие переходы и возвращает действия вызывающему сетевому слою.
use std::time::Duration;

/// Максимальное время ожидания ответа на входящий DM-звонок.
pub const INCOMING_CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Период, в течение которого звонок сохраняется при кратком переподключении.
pub const REBIND_GRACE: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallPhase {
    Ringing,
    Active,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallIdentity {
    pub dm_channel_id: Option<String>,
    pub federated_call_id: Option<String>,
    pub origin: Option<String>,
}

impl CallIdentity {
    /// Сравнение только по идентификаторам, которые присутствуют с обеих сторон.
    /// Неизвестный ID сам по себе не считается совпадением.
    pub fn matches(&self, other: &Self) -> bool {
        // Если оба события знают свой инстанс, разные origin никогда не совпадают.
        // Отсутствующий origin допустим для серверных событий старого формата.
        if let (Some(a), Some(b)) = (&self.origin, &other.origin) {
            if a != b {
                return false;
            }
        }
        let mut comparable = false;
        if let (Some(a), Some(b)) = (&self.dm_channel_id, &other.dm_channel_id) {
            comparable = true;
            if a != b {
                return false;
            }
        }
        if let (Some(a), Some(b)) = (&self.federated_call_id, &other.federated_call_id) {
            comparable = true;
            if a != b {
                return false;
            }
        }
        comparable
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRecord {
    pub identity: CallIdentity,
    pub peer_id: String,
    pub peer_name: String,
    pub phase: CallPhase,
    pub incoming: bool,
    pub since: Duration,
    pub disconnected_at: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolAction {
    SendAccept(CallIdentity),
    SendReject(CallIdentity),
    SendEnd(CallIdentity),
    IncomingTimedOut(CallIdentity),
    RebindExpired(CallIdentity),
    IgnoreStaleEvent,
}

#[derive(Debug, Default)]
pub struct VoiceCoordinator {
    channel_id: Option<String>,
    call: Option<CallRecord>,
    muted: bool,
    deafened: bool,
}

impl VoiceCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn channel_id(&self) -> Option<&str> {
        self.channel_id.as_deref()
    }
    pub fn call(&self) -> Option<&CallRecord> {
        self.call.as_ref()
    }
    pub fn is_muted(&self) -> bool {
        self.muted
    }
    pub fn is_deafened(&self) -> bool {
        self.deafened
    }

    /// Устанавливает локальный голосовой канал; `None` означает выход.
    pub fn set_channel(&mut self, channel_id: Option<String>) {
        self.channel_id = channel_id;
        if self.channel_id.is_none() {
            self.muted = false;
            self.deafened = false;
        }
    }

    pub fn set_mute_state(&mut self, muted: bool, deafened: bool) {
        self.muted = muted;
        self.deafened = deafened;
    }

    /// Регистрирует входящий звонок. Повторный звонок с тем же ID обновляет
    /// сведения, но не сбрасывает исходный дедлайн ответа.
    pub fn incoming_call(
        &mut self,
        identity: CallIdentity,
        peer_id: String,
        peer_name: String,
        now: Duration,
    ) -> bool {
        if identity.dm_channel_id.is_none() && identity.federated_call_id.is_none() {
            return false;
        }
        if let Some(existing) = self.call.as_mut() {
            if existing.identity.matches(&identity) {
                existing.identity = identity;
                existing.peer_id = peer_id;
                existing.peer_name = peer_name;
                existing.disconnected_at = None;
                return true;
            }
            if existing.phase != CallPhase::Ended {
                return false;
            }
        }
        self.call = Some(CallRecord {
            identity,
            peer_id,
            peer_name,
            phase: CallPhase::Ringing,
            incoming: true,
            since: now,
            disconnected_at: None,
        });
        true
    }

    /// Начинает исходящий звонок, не подменяя другой незавершённый вызов.
    pub fn outgoing_call(
        &mut self,
        identity: CallIdentity,
        peer_id: String,
        peer_name: String,
        now: Duration,
    ) -> bool {
        if identity.dm_channel_id.is_none() && identity.federated_call_id.is_none() {
            return false;
        }
        if self
            .call
            .as_ref()
            .is_some_and(|call| call.phase != CallPhase::Ended)
        {
            return false;
        }
        self.call = Some(CallRecord {
            identity,
            peer_id,
            peer_name,
            phase: CallPhase::Ringing,
            incoming: false,
            since: now,
            disconnected_at: None,
        });
        true
    }

    pub fn accept(&mut self, identity: &CallIdentity) -> Option<ProtocolAction> {
        let call = self.call.as_mut()?;
        if !call.incoming || call.phase != CallPhase::Ringing || !call.identity.matches(identity) {
            return Some(ProtocolAction::IgnoreStaleEvent);
        }
        call.phase = CallPhase::Active;
        Some(ProtocolAction::SendAccept(call.identity.clone()))
    }

    pub fn reject(&mut self, identity: &CallIdentity) -> Option<ProtocolAction> {
        let call = self.call.as_mut()?;
        if !call.identity.matches(identity) || call.phase == CallPhase::Ended {
            return Some(ProtocolAction::IgnoreStaleEvent);
        }
        call.phase = CallPhase::Ended;
        Some(ProtocolAction::SendReject(call.identity.clone()))
    }

    pub fn end(&mut self, identity: &CallIdentity) -> Option<ProtocolAction> {
        let call = self.call.as_mut()?;
        if !call.identity.matches(identity) || call.phase == CallPhase::Ended {
            return Some(ProtocolAction::IgnoreStaleEvent);
        }
        call.phase = CallPhase::Ended;
        Some(ProtocolAction::SendEnd(call.identity.clone()))
    }

    /// Сервер подтвердил принятие или отклонение. События чужого звонка игнорируются.
    pub fn server_accepted(&mut self, identity: &CallIdentity) -> bool {
        let Some(call) = self.call.as_mut() else {
            return false;
        };
        if !call.identity.matches(identity) || call.phase == CallPhase::Ended {
            return false;
        }
        call.phase = CallPhase::Active;
        true
    }

    pub fn server_ended(&mut self, identity: &CallIdentity) -> bool {
        let Some(call) = self.call.as_mut() else {
            return false;
        };
        if !call.identity.matches(identity) {
            return false;
        }
        call.phase = CallPhase::Ended;
        call.disconnected_at = None;
        true
    }

    /// При потере транспорта запускает grace-период; повторная привязка отменяет его.
    pub fn transport_lost(&mut self, now: Duration) {
        if let Some(call) = self.call.as_mut().filter(|c| c.phase != CallPhase::Ended) {
            call.disconnected_at = Some(now);
        }
    }

    pub fn rebind(&mut self) {
        if let Some(call) = self.call.as_mut() {
            call.disconnected_at = None;
        }
    }

    /// Обрабатывает дедлайны. Вызывающий код должен вызывать метод регулярно.
    pub fn tick(&mut self, now: Duration) -> Vec<ProtocolAction> {
        let Some(call) = self.call.as_mut() else {
            return Vec::new();
        };
        if call.phase == CallPhase::Ended {
            return Vec::new();
        }
        if let Some(lost_at) = call.disconnected_at {
            if now.saturating_sub(lost_at) >= REBIND_GRACE {
                call.phase = CallPhase::Ended;
                return vec![ProtocolAction::RebindExpired(call.identity.clone())];
            }
            return Vec::new();
        }
        if call.incoming
            && call.phase == CallPhase::Ringing
            && now.saturating_sub(call.since) >= INCOMING_CALL_TIMEOUT
        {
            call.phase = CallPhase::Ended;
            return vec![ProtocolAction::IncomingTimedOut(call.identity.clone())];
        }
        Vec::new()
    }

    pub fn clear(&mut self) {
        self.channel_id = None;
        self.call = None;
        self.muted = false;
        self.deafened = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(dm: Option<&str>, fed: Option<&str>) -> CallIdentity {
        CallIdentity {
            dm_channel_id: dm.map(str::to_owned),
            federated_call_id: fed.map(str::to_owned),
            origin: None,
        }
    }
    #[test]
    fn refuses_call_without_identifiers() {
        let mut c = VoiceCoordinator::new();
        assert!(!c.incoming_call(id(None, None), "u".into(), "User".into(), Duration::ZERO));
    }
    #[test]
    fn timeout_fires_at_sixty_seconds() {
        let mut c = VoiceCoordinator::new();
        let call = id(Some("dm1"), None);
        assert!(c.incoming_call(call.clone(), "u".into(), "User".into(), Duration::ZERO));
        assert!(c.tick(Duration::from_secs(59)).is_empty());
        assert_eq!(
            c.tick(Duration::from_secs(60)),
            vec![ProtocolAction::IncomingTimedOut(call)]
        );
    }
    #[test]
    fn unrelated_call_cannot_end_active_call() {
        let mut c = VoiceCoordinator::new();
        c.outgoing_call(
            id(Some("dm1"), Some("fed1")),
            "u".into(),
            "User".into(),
            Duration::ZERO,
        );
        assert!(!c.server_ended(&id(Some("dm2"), Some("fed2"))));
        assert_eq!(c.call().unwrap().phase, CallPhase::Ringing);
    }
    #[test]
    fn different_origins_never_match() {
        let mut home = id(Some("dm1"), Some("fed1"));
        home.origin = Some(String::new());
        let mut remote = home.clone();
        remote.origin = Some("https://chat.example.com".to_owned());
        assert!(!home.matches(&remote));
    }

    #[test]
    fn missing_origin_remains_compatible_with_legacy_events() {
        let mut known = id(Some("dm1"), None);
        known.origin = Some("https://chat.example.com".to_owned());
        assert!(known.matches(&id(Some("dm1"), None)));
    }

    #[test]
    fn requires_matching_available_identifiers() {
        assert!(id(Some("dm1"), Some("fed1")).matches(&id(Some("dm1"), None)));
        assert!(!id(Some("dm1"), Some("fed1")).matches(&id(None, Some("fed2"))));
        assert!(!id(Some("dm1"), None).matches(&id(Some("dm2"), None)));
    }
    #[test]
    fn rebind_cancels_grace_deadline() {
        let mut c = VoiceCoordinator::new();
        let call = id(Some("dm1"), None);
        c.outgoing_call(call, "u".into(), "User".into(), Duration::ZERO);
        c.transport_lost(Duration::from_secs(1));
        c.rebind();
        assert!(c.tick(Duration::from_secs(30)).is_empty());
    }
    #[test]
    fn grace_expiry_ends_call() {
        let mut c = VoiceCoordinator::new();
        let call = id(Some("dm1"), None);
        c.outgoing_call(call.clone(), "u".into(), "User".into(), Duration::ZERO);
        c.transport_lost(Duration::from_secs(1));
        assert_eq!(
            c.tick(Duration::from_secs(16)),
            vec![ProtocolAction::RebindExpired(call)]
        );
    }
}
