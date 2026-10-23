//! Классификация отказов WS-аутентификации.

/// Почему сервер отверг токен при auth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthFailure {
    Invalid,
    Revoked,
    AccountDeleted,
    Protocol,
    Other,
}

impl AuthFailure {
    /// Классифицирует текст ошибки сервера (подстрока, без регистра).
    #[must_use]
    pub fn classify(message: &str) -> Self {
        let lower = message.to_lowercase();
        if lower.contains("revoked") {
            Self::Revoked
        } else if lower.contains("deleted") {
            Self::AccountDeleted
        } else if lower.contains("invalid token") {
            Self::Invalid
        } else if lower.contains("timeout") || lower.contains("must be auth") {
            Self::Protocol
        } else {
            Self::Other
        }
    }

    /// Поможет ли новый токен.
    #[must_use]
    pub const fn is_recoverable(self) -> bool {
        matches!(self, Self::Invalid | Self::Revoked)
    }

    /// Фатальна ли причина.
    #[must_use]
    pub const fn is_fatal(self) -> bool {
        matches!(self, Self::AccountDeleted | Self::Protocol)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::AuthFailure;

    #[test]
    fn classifies_server_texts() {
        assert_eq!(AuthFailure::classify("Invalid token"), AuthFailure::Invalid);
        assert_eq!(
            AuthFailure::classify("Token has been revoked"),
            AuthFailure::Revoked
        );
        assert_eq!(
            AuthFailure::classify("This account has been deleted"),
            AuthFailure::AccountDeleted
        );
        assert_eq!(
            AuthFailure::classify("Authentication timeout"),
            AuthFailure::Protocol
        );
        assert_eq!(
            AuthFailure::classify("First message must be auth"),
            AuthFailure::Protocol
        );
        assert_eq!(
            AuthFailure::classify("Internal server error"),
            AuthFailure::Other
        );
    }

    #[test]
    fn revoked_wins_over_invalid() {
        assert_eq!(
            AuthFailure::classify("token revoked, invalid token"),
            AuthFailure::Revoked
        );
    }

    #[test]
    fn recoverable_and_fatal_are_disjoint() {
        for failure in [
            AuthFailure::Invalid,
            AuthFailure::Revoked,
            AuthFailure::AccountDeleted,
            AuthFailure::Protocol,
            AuthFailure::Other,
        ] {
            assert!(!(failure.is_recoverable() && failure.is_fatal()));
        }
        assert!(AuthFailure::Invalid.is_recoverable());
        assert!(AuthFailure::Revoked.is_recoverable());
        assert!(AuthFailure::AccountDeleted.is_fatal());
        assert!(AuthFailure::Protocol.is_fatal());
    }
}
