//! Высокоуровневый login-flow поверх `bs-api`.

use bs_api::ApiClient;
use bs_types::{Origin, TransportPolicy, User};

use crate::error::IdentityError;
use crate::secret::AccessToken;
use crate::store::CredentialStore;

/// Сессия одного инстанса: авторизованный клиент + профиль пользователя.
#[derive(Debug)]
pub struct InstanceSession {
    origin: Origin,
    client: ApiClient,
    token: AccessToken,
    user: User,
}

impl InstanceSession {
    /// Подключается к инстансу: логинится, сохраняет токен, тянет профиль.
    ///
    /// `base` — абсолютный URL инстанса без `/api` (например,
    /// `https://chat.example.com`). При успехе токен кладётся в `store`
    /// под ключом `origin`.
    pub async fn login(
        store: &mut CredentialStore,
        origin: Origin,
        base: &str,
        username: &str,
        password: &str,
    ) -> Result<Self, IdentityError> {
        Self::login_with_policy(
            store,
            origin,
            base,
            username,
            password,
            TransportPolicy::Strict,
        )
        .await
    }

    /// Как [`InstanceSession::login`], но с явной политикой транспорта.
    pub async fn login_with_policy(
        store: &mut CredentialStore,
        origin: Origin,
        base: &str,
        username: &str,
        password: &str,
        policy: TransportPolicy,
    ) -> Result<Self, IdentityError> {
        let mut client = ApiClient::with_policy(origin.clone(), base, policy)?;
        let auth = client.login_and_attach(username, password).await?;
        let token = AccessToken::new(auth.token);
        store.insert(origin.clone(), token.clone());
        Ok(Self {
            origin,
            client,
            token,
            user: auth.user,
        })
    }

    /// Восстанавливает сессию из сохранённого токена, проверяя его через
    /// `GET /api/users/@me`. При 401 токен удаляется из `store`.
    pub async fn resume(
        store: &mut CredentialStore,
        origin: Origin,
        base: &str,
    ) -> Result<Self, IdentityError> {
        Self::resume_with_policy(store, origin, base, TransportPolicy::Strict).await
    }

    /// Как [`InstanceSession::resume`], но с явной политикой транспорта.
    pub async fn resume_with_policy(
        store: &mut CredentialStore,
        origin: Origin,
        base: &str,
        policy: TransportPolicy,
    ) -> Result<Self, IdentityError> {
        let token = store.get(&origin)?.clone();
        let mut client = ApiClient::with_policy(origin.clone(), base, policy)?;
        client.set_token(Some(token.expose().to_owned()));
        match client.me().await {
            Ok(user) => Ok(Self {
                origin,
                client,
                token,
                user,
            }),
            Err(err) if err.is_unauthorized() => {
                store.remove(&origin);
                Err(IdentityError::TokenRejected)
            }
            Err(err) => Err(IdentityError::Api(err)),
        }
    }

    /// Инстанс сессии.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Авторизованный REST-клиент.
    pub fn client(&self) -> &ApiClient {
        &self.client
    }

    /// Изменяемый REST-клиент (для дальнейших запросов).
    pub fn client_mut(&mut self) -> &mut ApiClient {
        &mut self.client
    }

    /// Токен сессии.
    pub fn token(&self) -> &AccessToken {
        &self.token
    }

    /// Профиль вошедшего пользователя.
    pub fn user(&self) -> &User {
        &self.user
    }

    /// Заменяет токен сессии (например, после смены пароля на удалённом
    /// инстансе: сервер отзывает прежние сессии и выдаёт новый JWT).
    pub fn set_token(&mut self, token: AccessToken) {
        self.client.set_token(Some(token.expose().to_owned()));
        self.token = token;
    }

    /// Обновляет профиль (после re-attach или смены имени).
    pub fn set_user(&mut self, user: User) {
        self.user = user;
    }

    /// Разлогинивает: удаляет токен из хранилища.
    pub fn logout(self, store: &mut CredentialStore) {
        store.remove(&self.origin);
    }
}
