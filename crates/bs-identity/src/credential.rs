//! Per-remote федеративный креденшел (`ensureRemoteCredential`).
//!
//! Порт `ensureRemoteCredential` из `packages/web/src/stores/instanceStore.ts`.
//! Удалённый аккаунт пользователя должен аутентифицироваться выданным домом
//! секретом, а не паролем домашнего аккаунта. Секрет выдаётся **только**
//! домашним инстансом (иначе один и тот же аккаунт получил бы разные секреты
//! с разных устройств и пользователь потерял бы доступ), хранится там
//! идемпотентно и переиспользуется при повторных вызовах.
//!
//! # Пример
//! ```no_run
//! # async fn run() -> Result<(), bs_identity::IdentityError> {
//! use bs_api::ApiClient;
//! use bs_identity::{CredentialOutcome, InstanceSession};
//! use bs_types::Origin;
//!
//! # async fn demo(home: &ApiClient, remote: &mut InstanceSession) -> Result<(), bs_identity::IdentityError> {
//! let outcome = remote.ensure_remote_credential(home, false).await?;
//! match outcome {
//!     CredentialOutcome::AlreadyProvisioned => {}
//!     CredentialOutcome::Rotated { token } => {
//!         let _ = token.expose();
//!     }
//!     CredentialOutcome::Skipped(_) => {}
//!     _ => {}
//! }
//! # Ok(())
//! # }
//! # Ok(())
//! # }
//! ```

use bs_api::{ApiClient, ChangePasswordRequest, FederationCredentialRequest};
use bs_types::Origin;

use crate::error::IdentityError;
use crate::identity::{IdentityFields, home_host_of, home_identity_of, host_of_url};
use crate::secret::AccessToken;
use crate::session::InstanceSession;

/// Почему креденшел трогать не стали.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// Удалённый инстанс — это домашний инстанс: там настоящий пароль.
    HomeInstance,
    /// Аккаунт на удалённом инстансе — не федеративная копия этого пользователя.
    NotMyIdentity,
}

/// Результат [`InstanceSession::ensure_remote_credential`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CredentialOutcome {
    /// Секрет уже применён (`provisioned`), ротация не нужна.
    AlreadyProvisioned,
    /// Пароль удалённого аккаунта заменён выданным секретом.
    Rotated {
        /// Новый токен удалённой сессии: прежний отозван сменой пароля.
        token: AccessToken,
    },
    /// Креденшел не относится к этому аккаунту — ничего не меняли.
    Skipped(SkipReason),
}

impl InstanceSession {
    /// Приводит учётные данные удалённого аккаунта к выданному домом секрету.
    ///
    /// `home` — авторизованный клиент **домашнего** инстанса этого
    /// пользователя. `force` пропускает проверку `provisioned`; его ставят
    /// после явного логина, который мог перезаписать секрет другим паролем.
    ///
    /// При успешной ротации токен сессии обновляется: сервер отзывает прочие
    /// сессии аккаунта при смене пароля, и старый JWT перестаёт работать.
    ///
    /// # Ошибки
    /// - [`IdentityError::NotHomeIssuer`] — `home` не является домашним клиентом;
    /// - [`IdentityError::RemoteOriginRequired`] — сессия сама домашняя;
    /// - [`IdentityError::Api`] — сервер отклонил выдачу секрета или смену пароля.
    pub async fn ensure_remote_credential(
        &mut self,
        home: &ApiClient,
        force: bool,
    ) -> Result<CredentialOutcome, IdentityError> {
        if !home.origin().is_home() {
            return Err(IdentityError::NotHomeIssuer);
        }
        if self.origin().is_home() {
            return Ok(CredentialOutcome::Skipped(SkipReason::HomeInstance));
        }
        let base = self.client().base().to_owned();
        let origin_str = self.origin().as_web_str();

        // Хост домашнего инстанса — то, чем должна быть "родина" удалённой записи.
        let home_host = home
            .base()
            .parse::<url::Url>()
            .map_or_else(|_| home_host_of(home.base()), |url| host_of_url(&url));
        let fields = IdentityFields::of_user(self.user());
        // Аккаунт на удалённом инстансе — федеративная копия именно этого
        // пользователя, только если его дом совпадает с нашим домашним.
        let Some(identity) = home_identity_of(&fields, &origin_str, &home_host) else {
            return Ok(CredentialOutcome::Skipped(SkipReason::NotMyIdentity));
        };
        if identity.host != home_host {
            return Ok(CredentialOutcome::Skipped(SkipReason::NotMyIdentity));
        }

        let credential = home
            .federation_credential(&FederationCredentialRequest::new(base.clone()))
            .await?;
        if credential.provisioned && !force {
            return Ok(CredentialOutcome::AlreadyProvisioned);
        }

        let rotated = self
            .client()
            .change_password(&ChangePasswordRequest::new(credential.secret.clone()))
            .await?;
        let token = AccessToken::new(rotated.token);
        self.set_token(token.clone());

        // Отметка ставится после ротации: если она не пройдёт, следующий
        // вызов просто повторит ротацию с тем же (идемпотентным) секретом.
        home.federation_credential(&FederationCredentialRequest::marking_provisioned(base))
            .await?;

        Ok(CredentialOutcome::Rotated { token })
    }

    /// Origin сессии, для которой выдаётся секрет.
    #[must_use]
    pub fn credential_origin(&self) -> &Origin {
        self.origin()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn skip_reasons_are_distinct() {
        assert_ne!(SkipReason::HomeInstance, SkipReason::NotMyIdentity);
    }

    #[test]
    fn outcome_carries_rotated_token() {
        let outcome = CredentialOutcome::Rotated {
            token: AccessToken::new("jwt"),
        };
        assert!(matches!(outcome, CredentialOutcome::Rotated { .. }));
    }
}
