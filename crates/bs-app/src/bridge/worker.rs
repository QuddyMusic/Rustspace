//! Фоновая tokio-задача сессии: вход, события, REST-запросы.

#![allow(
    clippy::wildcard_imports,
    reason = "части одного модуля, разнесённые по файлам"
)]

use super::*;

/// Параметры входа, переданные фоновой сессии.
pub(super) struct SessionParams {
    pub(super) origin: Origin,
    pub(super) base: String,
    pub(super) username: String,
    /// Пароль затирается в памяти, как только логин выполнен.
    pub(super) password: Zeroizing<String>,
    /// Код двухфакторной защиты, если пользователь его ввёл.
    pub(super) second_factor: Option<Zeroizing<String>>,
    /// Регистрация перед входом: федеративный аккаунт на чужом инстансе
    /// (отказ «занято» не мешает войти) или новый аккаунт на домашнем
    /// (любой отказ останавливает вход).
    pub(super) register: Option<RegisterRequest>,
    /// Сохранённый токен: вход без пароля.
    pub(super) resume: Option<Secret>,
    /// Файл, куда писать токен после успешного входа (только домашний инстанс).
    pub(super) persist: Option<std::path::PathBuf>,
    pub(super) transport: TransportPolicy,
}

/// Действие с двухфакторной защитой и свежее состояние после него.
async fn two_factor(client: &bs_api::ApiClient, op: TwoFactorOp) -> Result<TwoFactorView, String> {
    let text =
        |err: &bs_api::ApiError| crate::logic::describe_server_error(err.code(), &err.to_string());
    let mut view = TwoFactorView::default();
    let acted = match op {
        TwoFactorOp::Status => Ok(()),
        TwoFactorOp::Setup(password) => client
            .two_factor_setup(password.expose())
            .await
            .map(|setup| view.setup = Some(setup)),
        TwoFactorOp::Enable(code) => client
            .two_factor_enable(&code)
            .await
            .map(|codes| view.codes = Some(codes)),
        TwoFactorOp::Disable(password, code) => {
            client.two_factor_disable(password.expose(), &code).await
        }
        TwoFactorOp::NewCodes(password, code) => client
            .two_factor_recovery_codes(password.expose(), &code)
            .await
            .map(|codes| view.codes = Some(codes)),
    };
    acted.map_err(|err| text(&err))?;
    match client.two_factor_status().await {
        Ok(status) => view.status = Some(status),
        // Оригинальный сервер таких маршрутов не знает.
        Err(bs_api::ApiError::Server { status, .. }) if status.as_u16() == 404 => {
            view.status = None;
        }
        Err(err) => return Err(text(&err)),
    }
    Ok(view)
}

/// Фоновая сессия: логин, затем пересылка событий и команд.
pub(super) async fn run_session(
    params: SessionParams,
    events: std_mpsc::Sender<BridgeEvent>,
    mut commands: tokio_mpsc::UnboundedReceiver<ClientEvent>,
    mut requests: tokio_mpsc::UnboundedReceiver<BridgeRequest>,
    responses: std_mpsc::Sender<BridgeResponse>,
) {
    // Сработает при любом выходе из функции, включая `abort()` и панику.
    let _end = EndGuard(events.clone());
    let persist = params.persist.clone();
    let Some(handle) = sign_in(params, &events).await else {
        return;
    };
    let mut sub = handle.subscribe();
    // Загрузки картинок идут параллельно, но не больше `MAX_IMAGE_JOBS`
    // сразу; при выходе из сессии `JoinSet` обрывает незавершённые.
    let mut jobs: bs_rt::JoinSet<()> = bs_rt::JoinSet::new();
    let gate = std::sync::Arc::new(bs_rt::sync::Semaphore::new(MAX_IMAGE_JOBS));
    loop {
        bs_rt::select! {
            Some(_) = jobs.join_next(), if !jobs.is_empty() => {}
            event = sub.recv() => {
                if !forward_event(event, &events, persist.as_deref()) {
                    return;
                }
            }
            request = requests.recv() => {
                let Some(request) = request else {
                    return;
                };
                let Some(request) =
                    spawn_job(request, handle.client(), &gate, &responses, &mut jobs)
                else {
                    continue;
                };
                let response = handle_request(handle.client(), request).await;
                if responses.send(response).is_err() {
                    return;
                }
            }
            command = commands.recv() => {
                let Some(event) = command else {
                    return;
                };
                if let Err(err) = handle.send(event).await {
                    let _ = events.send(BridgeEvent::Error { message: err.to_string() });
                }
            }
        }
    }
}

/// Регистрация (если нужна), вход, `ready` и сохранение сессии.
/// `None` — вход не удался, GUI уже знает почему.
async fn sign_in(
    params: SessionParams,
    events: &std_mpsc::Sender<BridgeEvent>,
) -> Option<InstanceHandle> {
    let SessionParams {
        origin,
        base,
        username,
        password,
        second_factor,
        register,
        resume,
        persist,
        transport,
    } = params;
    let mut refusal = None;
    if let Some(request) = register.as_ref().filter(|_| origin == Origin::Home) {
        if !register_home(&base, request, transport, events).await {
            return None;
        }
    } else if let Some(request) = register {
        refusal = try_register(&origin, &base, &request, transport, events).await;
        if refusal == Some(RegisterOutcome::Failed) {
            return None;
        }
    }
    let mut store = CredentialStore::new();
    let resuming = resume.is_some();
    let login = if let Some(token) = resume {
        store.insert(
            origin.clone(),
            bs_identity::AccessToken::new(token.expose()),
        );
        InstanceHandle::resume_with_transport(
            &mut store,
            origin,
            &base,
            Some(ClientKind::Desktop),
            ReconnectPolicy::default(),
            transport,
        )
        .await
    } else {
        InstanceHandle::login_with_factor(
            &mut store,
            origin,
            &base,
            &username,
            &password,
            second_factor.as_ref().map(|code| code.as_str()),
            Some(ClientKind::Desktop),
            ReconnectPolicy::default(),
            transport,
        )
        .await
    };
    drop(password);
    drop(second_factor);

    let (handle, ready) = match login {
        Ok(pair) => pair,
        Err(err) => {
            report_login_failure(&err, refusal, resuming, persist.as_deref(), events);
            return None;
        }
    };
    match ready {
        ServerEvent::Ready(payload) => {
            if events.send(BridgeEvent::Ready { payload }).is_err() {
                return None;
            }
        }
        other => {
            let _ = events.send(BridgeEvent::Error {
                message: format!("ожидался ready, пришло {other:?}"),
            });
            return None;
        }
    }
    if let Some(path) = persist.as_ref()
        && let Err(err) =
            crate::saved_session::save(path, &base, handle.token().expose(), transport)
    {
        tracing::warn!(%err, "не удалось сохранить сессию");
    }
    Some(handle)
}

/// Сообщает GUI, почему вход не удался.
fn report_login_failure(
    err: &bs_instances::InstanceError,
    refusal: Option<RegisterOutcome>,
    resuming: bool,
    persist: Option<&std::path::Path>,
    events: &std_mpsc::Sender<BridgeEvent>,
) {
    let rejected = matches!(
        err,
        bs_instances::InstanceError::Identity(bs_identity::IdentityError::TokenRejected)
    );
    if rejected && let Some(path) = persist {
        crate::saved_session::clear(path);
    }
    if resuming && rejected {
        let _ = events.send(BridgeEvent::LoginFailed {
            message: tr("Сессия истекла, войдите заново").to_owned(),
            kind: FailureKind::Auth,
        });
        return;
    }
    let message = if refusal.is_some() {
        remote_login_refused(refusal, &err.to_string())
    } else if let bs_instances::InstanceError::Identity(bs_identity::IdentityError::Api(api)) = err
        && let Some(text) = login_refusal(api.code())
    {
        text.to_owned()
    } else if let bs_instances::InstanceError::Identity(bs_identity::IdentityError::Api(api)) = err
        && api
            .code()
            .is_some_and(|code| code.starts_with("two_factor_"))
    {
        crate::logic::describe_server_error(api.code(), &err.to_string())
    } else if is_network_error(err) {
        // Технические подробности — в журнал, человеку — что делать.
        tracing::warn!(%err, "не удалось подключиться к инстансу");
        tr("Не удаётся связаться с сервером. Проверьте адрес инстанса и подключение к сети.")
            .to_owned()
    } else {
        err.to_string()
    };
    let kind = match refusal {
        Some(RegisterOutcome::Taken) => FailureKind::CredentialRefused,
        Some(RegisterOutcome::Closed) => FailureKind::RegistrationClosed,
        _ if is_network_error(err) => FailureKind::Network,
        _ => FailureKind::Auth,
    };
    let _ = events.send(BridgeEvent::LoginFailed { message, kind });
}

/// Отказ сервера на вход понятными словами.
fn login_refusal(code: Option<&str>) -> Option<&'static str> {
    Some(match code? {
        "invalid_credentials" => tr("Неверный логин или пароль"),
        "account_deleted" => tr("Этот аккаунт удалён"),
        "rate_limited" => tr("Слишком много попыток: подождите немного и попробуйте снова"),
        _ => return None,
    })
}

/// Пересылает событие инстанса в GUI; `false` — сессию пора завершать.
fn forward_event(
    event: Result<InstanceEvent, broadcast::error::RecvError>,
    events: &std_mpsc::Sender<BridgeEvent>,
    persist: Option<&std::path::Path>,
) -> bool {
    match event {
        Ok(InstanceEvent::Server(boxed)) => {
            events.send(BridgeEvent::Server { event: boxed }).is_ok()
        }
        Ok(InstanceEvent::AuthRejected {
            failure, message, ..
        }) => {
            // Токен больше не годится: в следующий раз войти по нему нельзя.
            if let Some(path) = persist {
                crate::saved_session::clear(path);
            }
            let _ = events.send(BridgeEvent::Error {
                message: format!("{message} ({failure:?})"),
            });
            true
        }
        Ok(InstanceEvent::StateChanged(state)) => events.send(BridgeEvent::State { state }).is_ok(),
        Err(broadcast::error::RecvError::Lagged(skipped)) => {
            tracing::warn!(skipped, "пропущены события инстанса");
            true
        }
        Err(broadcast::error::RecvError::Closed) => {
            let _ = events.send(BridgeEvent::Disconnected);
            false
        }
    }
}

/// Картинки, скачивания, загрузки файлов, настройки и прочие независимые
/// чтения идут параллельно в `jobs`. Остальные запросы (сообщения, правки —
/// там важен порядок) возвращаются для обработки по очереди.
fn spawn_job(
    request: BridgeRequest,
    client: &bs_api::ApiClient,
    gate: &std::sync::Arc<bs_rt::sync::Semaphore>,
    responses: &std_mpsc::Sender<BridgeResponse>,
    jobs: &mut bs_rt::JoinSet<()>,
) -> Option<BridgeRequest> {
    let client = client.clone();
    let tx = responses.clone();
    match request {
        BridgeRequest::Image(path) => {
            let gate = std::sync::Arc::clone(gate);
            jobs.spawn(async move {
                let Ok(_permit) = gate.acquire_owned().await else {
                    return;
                };
                let result = load_image(&client, &path).await;
                let _ = tx.send(BridgeResponse::Image { path, result });
            });
        }
        BridgeRequest::Download { id, path, dest }
            if crate::e2ee_files::key_for(path.as_str()).is_some() =>
        {
            jobs.spawn(async move {
                let state = download_sealed(&client, &path, &dest).await;
                let _ = tx.send(BridgeResponse::Download { id, state });
            });
        }
        // В браузере файл целиком скачивается и отдаётся браузеру.
        BridgeRequest::Download { id, path, dest } => {
            jobs.spawn(download_job(client, id, path, dest, tx));
        }
        BridgeRequest::Raw { tag, path } => {
            jobs.spawn(async move {
                let result = client
                    .fetch_asset(&path, bs_media::Limits::default().max_input_bytes)
                    .await
                    .map_err(|err| err.to_string());
                let _ = tx.send(BridgeResponse::Raw { tag, result });
            });
        }
        BridgeRequest::Upload {
            id,
            name,
            mime,
            bytes,
        } => {
            jobs.spawn(async move {
                let progress_tx = tx.clone();
                let result = client
                    .upload_file(&name, &mime, &bytes, move |sent, total| {
                        let _ = progress_tx.send(BridgeResponse::Compose(ComposeEvent::Progress {
                            id,
                            sent,
                            total,
                        }));
                    })
                    .await
                    .map(Box::new)
                    .map_err(|err| upload_message(&err));
                let _ = tx.send(BridgeResponse::Compose(ComposeEvent::Uploaded {
                    id,
                    result,
                }));
            });
        }
        BridgeRequest::SetImage { kind, name, bytes } => {
            jobs.spawn(async move {
                let response = set_profile_image(&client, kind, &name, &bytes).await;
                let _ = tx.send(response);
            });
        }
        // Чтения, настройки и превью трансляций не стоят за медленным
        // запросом (мосты, история): разделы открываются сразу.
        request @ (BridgeRequest::Admin(_)
        | BridgeRequest::Extras(_)
        | BridgeRequest::InstanceInfo
        | BridgeRequest::Directory(_)
        | BridgeRequest::GifEnabled
        | BridgeRequest::Gifs { .. }
        | BridgeRequest::Stickers
        | BridgeRequest::StreamPreview(_)
        | BridgeRequest::Profile(_)) => {
            jobs.spawn(async move {
                let response = handle_request(&client, request).await;
                let _ = tx.send(response);
            });
        }
        other => return Some(other),
    }
    None
}

/// Скачивание в файл с ходом загрузки.
#[cfg(not(target_arch = "wasm32"))]
async fn download_job(
    client: bs_api::ApiClient,
    id: u64,
    path: bs_api::AssetPath,
    dest: std::path::PathBuf,
    tx: std_mpsc::Sender<BridgeResponse>,
) {
    let mut reported = 0_u64;
    let progress_tx = tx.clone();
    let result = client
        .download_asset(&path, &dest, move |got, total| {
            // Не чаще раза на каждые 128 КиБ (и в самом начале).
            if got == 0 || got.saturating_sub(reported) >= DOWNLOAD_STEP {
                reported = got;
                let _ = progress_tx.send(BridgeResponse::Download {
                    id,
                    state: DownloadState::Progress { got, total },
                });
            }
        })
        .await;
    let state = match result {
        Ok(_) => DownloadState::Done,
        Err(err) => DownloadState::Failed(download_message(&err)),
    };
    let _ = tx.send(BridgeResponse::Download { id, state });
}

/// Скачивание в браузере: файл целиком и сразу браузеру.
#[cfg(target_arch = "wasm32")]
async fn download_job(
    client: bs_api::ApiClient,
    id: u64,
    path: bs_api::AssetPath,
    dest: std::path::PathBuf,
    tx: std_mpsc::Sender<BridgeResponse>,
) {
    let result = async {
        let bytes = client
            .fetch_asset(&path, usize::MAX)
            .await
            .map_err(|err| download_message(&err))?;
        crate::sys::save_file(&dest, bytes).await
    }
    .await;
    let state = result.map_or_else(DownloadState::Failed, |()| DownloadState::Done);
    let _ = tx.send(BridgeResponse::Download { id, state });
}

/// Чем закончилась попытка регистрации на чужом инстансе.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RegisterOutcome {
    /// Аккаунт создан.
    Created,
    /// Имя занято: аккаунт, возможно, уже есть.
    Taken,
    /// Инстанс не принимает новых федеративных аккаунтов.
    Closed,
    /// Другая ошибка; текст уже отправлен в GUI.
    Failed,
}

/// Сетевая ли это ошибка (инстанс не ответил), а не отказ сервера.
pub(super) fn is_network_error(err: &bs_instances::InstanceError) -> bool {
    use bs_api::ApiError;
    use bs_identity::IdentityError;
    use bs_instances::InstanceError;
    matches!(
        err,
        InstanceError::Api(ApiError::Transport(_))
            | InstanceError::Identity(IdentityError::Api(ApiError::Transport(_)))
            | InstanceError::Ws(_)
    )
}

/// Пространство целиком (каналы, участники, роли) для кэша.
pub(super) async fn full_space(client: &bs_api::ApiClient, space_id: &str) -> BridgeResponse {
    match client.space(space_id).await {
        Ok(space) => BridgeResponse::Space(Box::new(space)),
        Err(err) => BridgeResponse::Failed(err.to_string()),
    }
}

/// Создаёт аккаунт на домашнем инстансе. Любой отказ — конец попытки: GUI
/// получает понятное объяснение.
pub(super) async fn register_home(
    base: &str,
    request: &RegisterRequest,
    transport: TransportPolicy,
    events: &std_mpsc::Sender<BridgeEvent>,
) -> bool {
    let result = match ApiClient::with_policy(Origin::Home, base, transport) {
        Ok(client) => client.register(request).await,
        Err(err) => Err(err),
    };
    let Err(err) = result else {
        return true;
    };
    let kind = if matches!(err, bs_api::ApiError::Transport(_)) {
        FailureKind::Network
    } else {
        FailureKind::Auth
    };
    let _ = events.send(BridgeEvent::LoginFailed {
        message: crate::logic::register_error_text(&err),
        kind,
    });
    false
}

/// Пытается создать аккаунт; отказ «занято/закрыто» не ошибка, дальше идёт вход.
pub(super) async fn try_register(
    origin: &Origin,
    base: &str,
    request: &RegisterRequest,
    transport: TransportPolicy,
    events: &std_mpsc::Sender<BridgeEvent>,
) -> Option<RegisterOutcome> {
    let client = match ApiClient::with_policy(origin.clone(), base, transport) {
        Ok(client) => client,
        Err(err) => {
            let _ = events.send(BridgeEvent::Error {
                message: err.to_string(),
            });
            return Some(RegisterOutcome::Failed);
        }
    };
    match client.register(request).await {
        Ok(_) => Some(RegisterOutcome::Created),
        Err(bs_api::ApiError::Server { status, .. }) if status.as_u16() == 409 => {
            Some(RegisterOutcome::Taken)
        }
        Err(bs_api::ApiError::Server { status, .. }) if status.as_u16() == 403 => {
            Some(RegisterOutcome::Closed)
        }
        Err(err) => {
            let kind = if matches!(err, bs_api::ApiError::Transport(_)) {
                FailureKind::Network
            } else {
                FailureKind::Auth
            };
            let _ = events.send(BridgeEvent::LoginFailed {
                message: err.to_string(),
                kind,
            });
            Some(RegisterOutcome::Failed)
        }
    }
}

/// Объяснение отказа входа на чужой инстанс: что именно можно утверждать.
///
/// Про существующий аккаунт говорим только после ответа «имя занято»:
/// закрытый инстанс одинаково отвечает на вход и за несуществующий
/// аккаунт, и за аккаунт с другим паролем.
pub(super) fn remote_login_refused(outcome: Option<RegisterOutcome>, raw: &str) -> String {
    match outcome {
        Some(RegisterOutcome::Taken) => "На этом инстансе уже есть аккаунт с таким именем, \
            но он не принимает пароль, выданный вашим домашним инстансом."
            .to_owned(),
        Some(RegisterOutcome::Closed) => "Инстанс не принимает новые аккаунты с других \
            инстансов. Если аккаунт у вас там уже есть, он не принял выданный пароль."
            .to_owned(),
        _ => raw.to_owned(),
    }
}

/// Сколько картинок качается и декодируется одновременно.
pub(super) const MAX_IMAGE_JOBS: usize = 6;

/// Скачивает и декодирует картинку; ошибка — готовый текст для показа.
/// Зашифрованное вложение: скачать, расшифровать, записать.
async fn download_sealed(
    client: &bs_api::ApiClient,
    path: &bs_api::AssetPath,
    dest: &std::path::Path,
) -> DownloadState {
    let result = async {
        let bytes = client
            .fetch_asset(path, usize::MAX)
            .await
            .map_err(|err| download_message(&err))?;
        let plain = crate::e2ee_files::reveal(path.as_str(), bytes)?;
        crate::sys::save_file(dest, plain).await
    }
    .await;
    match result {
        Ok(()) => DownloadState::Done,
        Err(message) => DownloadState::Failed(message),
    }
}

pub(super) async fn load_image(
    client: &bs_api::ApiClient,
    path: &bs_api::AssetPath,
) -> Result<bs_media::Decoded, String> {
    let limits = bs_media::Limits::default();
    let bytes = client
        .fetch_asset(path, limits.max_input_bytes)
        .await
        .map_err(|err| err.to_string())?;
    // Вложение зашифрованной переписки — расшифровать перед показом.
    let bytes = crate::e2ee_files::reveal(path.as_str(), bytes)?;
    // Декодирование грузит процессор — уводим его с рабочих потоков сети.
    bs_rt::spawn_blocking(move || bs_media::decode(&bytes, &limits))
        .await
        .map_err(|err| crate::tf!("декодер упал: {err}", err = err))?
        .map_err(|err| err.to_string())
}

/// Выполняет REST-запрос и превращает результат в [`BridgeResponse`].
/// Проверяет пароль (если он есть) и просит у дома секрет для входа на `origin`.
pub(super) async fn issue_credential(
    client: &ApiClient,
    origin: &str,
    password: Option<&Secret>,
) -> BridgeResponse {
    let result = issue_credential_inner(client, origin, password).await;
    BridgeResponse::Credential {
        origin: origin.to_owned(),
        result,
    }
}

pub(super) async fn issue_credential_inner(
    client: &ApiClient,
    origin: &str,
    password: Option<&Secret>,
) -> Result<IssuedCredential, String> {
    let me = client.me().await.map_err(|err| err.to_string())?;
    let home_host = me
        .home_instance
        .clone()
        .unwrap_or_else(|| bs_identity::identity::host_of(client.base()));
    if bs_identity::identity::host_of(origin) == home_host {
        return Err(tr("Это ваш домашний инстанс: он уже подключён").to_owned());
    }
    if let Some(password) = password
        && !client
            .verify_password(password.expose())
            .await
            .map_err(|err| err.to_string())?
    {
        return Err(tr("Неверный пароль").to_owned());
    }
    let issued = client
        .federation_credential(&FederationCredentialRequest::new(origin))
        .await
        .map_err(|err| err.to_string())?;
    let bare = me.username.split('@').next().unwrap_or(&me.username);
    Ok(IssuedCredential {
        username: format!("{bare}@{home_host}"),
        secret: Secret::new(issued.secret),
        home_host,
        home_user_id: me.home_user_id.clone().unwrap_or_else(|| me.id.clone()),
        display_name: me.display_name.clone(),
        provisioned: issued.provisioned,
    })
}

/// Загружает картинку профиля и привязывает её к аккаунту.
pub(super) async fn set_profile_image(
    client: &bs_api::ApiClient,
    kind: ImageKind,
    name: &str,
    bytes: &[u8],
) -> BridgeResponse {
    let fail = |message: String| {
        BridgeResponse::Admin(AdminEvent::Done {
            action: AdminDone::Image,
            result: Err(message),
        })
    };
    let file = if bytes.is_empty() {
        String::new()
    } else {
        let mime = crate::media::mime_for_name(name);
        match client.upload_file(name, mime, bytes, |_, _| {}).await {
            Ok(attachment) => attachment.filename,
            Err(err) => return fail(upload_message(&err)),
        }
    };
    let mut request = bs_api::UpdateProfileRequest::default();
    match kind {
        ImageKind::Avatar => request.avatar = Some(file),
        ImageKind::Banner => request.banner = Some(file),
    }
    match client.update_me(&request).await {
        Ok(user) => BridgeResponse::Profile(Box::new(user)),
        Err(err) => fail(upload_message(&err)),
    }
}

/// Выполняет административное действие и, если нужно, перечитывает пространство.
#[allow(clippy::too_many_lines, reason = "один match по всем действиям")]
pub(super) async fn space_admin(
    client: &bs_api::ApiClient,
    space_id: String,
    op: SpaceOp,
) -> BridgeResponse {
    if op == SpaceOp::ListBans {
        let result = client
            .list_bans(&space_id)
            .await
            .map_err(|err| space_message(&err));
        return BridgeResponse::SpaceAction(SpaceEvent::Bans { space_id, result });
    }
    if let SpaceOp::ListOverrides(scope) = &op {
        let result = client
            .list_overrides(scope)
            .await
            .map_err(|err| space_message(&err));
        return BridgeResponse::SpaceAction(SpaceEvent::Overrides {
            space_id,
            scope: scope.clone(),
            result,
        });
    }
    if let SpaceOp::Notify(channel, change) = &op {
        let result = match channel {
            Some(id) => client.update_channel_notifications(id, change).await,
            None => client.update_space_notifications(&space_id, change).await,
        }
        .map_err(|err| space_message(&err));
        return BridgeResponse::SpaceAction(SpaceEvent::Notified { space_id, result });
    }
    if op == SpaceOp::ListJoinRequests {
        let result = client
            .pending_join_requests(&space_id)
            .await
            .map_err(|err| space_message(&err));
        return BridgeResponse::SpaceAction(SpaceEvent::Requests { space_id, result });
    }
    if let SpaceOp::UploadImage(banner, name, bytes) = &op {
        let mime = crate::media::mime_for_name(name);
        let result = client
            .upload_file(name, mime, bytes, |_, _| {})
            .await
            .map(|attachment| attachment.filename)
            .map_err(|err| upload_message(&err));
        return BridgeResponse::SpaceAction(SpaceEvent::Uploaded {
            space_id,
            banner: *banner,
            result,
        });
    }
    let done = match &op {
        SpaceOp::UpdateChannel(id, update) => client.update_channel(id, update).await,
        SpaceOp::DeleteChannel(id) => client.delete_channel(id).await,
        SpaceOp::RenameCategory(id, name) => client.rename_category(id, name).await,
        SpaceOp::DeleteCategory(id) => client.delete_category(id).await,
        SpaceOp::PutOverride(scope, entry) => client.put_override(scope, entry).await,
        SpaceOp::DeleteOverride(scope, kind, id) => client.delete_override(scope, *kind, id).await,
        SpaceOp::SetNsfw(channel, nsfw) => client.set_channel_nsfw(channel, *nsfw).await,
        SpaceOp::DecideRequest(id, accept) => {
            client.decide_join_request(&space_id, id, *accept).await
        }
        SpaceOp::Update(patch) => client.update_space(&space_id, patch).await,
        SpaceOp::Delete => client.delete_space(&space_id).await,
        SpaceOp::Kick(user) => client.leave_space(&space_id, user).await,
        SpaceOp::Ban(user, reason) => client.ban_member(&space_id, user, reason.as_deref()).await,
        SpaceOp::Unban(user) => client.unban_member(&space_id, user).await,
        SpaceOp::Transfer(user) => client.transfer_ownership(&space_id, user).await,
        SpaceOp::CreateRole(role) => client.create_role(&space_id, role).await.map(|_| ()),
        SpaceOp::UpdateRole(role_id, update) => client
            .update_role(&space_id, role_id, update)
            .await
            .map(|_| ()),
        SpaceOp::DeleteRole(role_id) => client.delete_role(&space_id, role_id).await,
        SpaceOp::AddMemberRole(user, role) => client.add_member_role(&space_id, user, role).await,
        SpaceOp::RemoveMemberRole(user, role) => {
            client.remove_member_role(&space_id, user, role).await
        }
        SpaceOp::ListBans
        | SpaceOp::ListJoinRequests
        | SpaceOp::ListOverrides(_)
        | SpaceOp::ListBridges(_)
        | SpaceOp::AddBridge(..)
        | SpaceOp::RemoveBridge(_)
        | SpaceOp::ListWebhooks(_)
        | SpaceOp::CreateWebhook(..)
        | SpaceOp::DeleteWebhook(_)
        | SpaceOp::Notify(..)
        | SpaceOp::UploadImage(..) => Ok(()),
    };
    let refresh = done.is_ok()
        && !matches!(
            op,
            SpaceOp::Delete | SpaceOp::Unban(_) | SpaceOp::DecideRequest(_, false)
        );
    let space = if refresh {
        client.space(&space_id).await.ok().map(Box::new)
    } else {
        None
    };
    BridgeResponse::SpaceAction(SpaceEvent::Admin {
        space_id,
        op,
        result: done.map_err(|err| space_message(&err)),
        space,
    })
}

/// Связывает или развязывает канал с внешним чатом.
async fn bridge_op(client: &bs_api::ApiClient, space_id: String, op: SpaceOp) -> BridgeResponse {
    let done = match &op {
        SpaceOp::ListBridges(channel_id) => {
            let result = client
                .channel_bridges(channel_id)
                .await
                .map_err(|err| space_message(&err));
            return BridgeResponse::SpaceAction(SpaceEvent::Bridges {
                space_id,
                channel_id: channel_id.clone(),
                result,
            });
        }
        SpaceOp::AddBridge(channel, kind, chat) => {
            client.add_channel_bridge(channel, kind, chat).await
        }
        SpaceOp::RemoveBridge(link) => client.remove_bridge(link).await,
        SpaceOp::ListWebhooks(channel_id) => {
            let result = client
                .webhooks(channel_id)
                .await
                .map_err(|err| space_message(&err));
            return BridgeResponse::SpaceAction(SpaceEvent::Webhooks {
                space_id,
                channel_id: channel_id.clone(),
                result,
            });
        }
        SpaceOp::CreateWebhook(channel, name) => {
            client.create_webhook(channel, name).await.map(|_| ())
        }
        SpaceOp::DeleteWebhook(hook) => client.delete_webhook(hook).await,
        _ => Ok(()),
    };
    BridgeResponse::SpaceAction(SpaceEvent::Admin {
        space_id,
        op,
        result: done.map_err(|err| space_message(&err)),
        space: None,
    })
}

/// Выполняет действие с инвайтом.
pub(super) async fn invite_op(client: &bs_api::ApiClient, op: InviteOp) -> Result<String, String> {
    let fail = |err: bs_api::ApiError| upload_message(&err);
    match op {
        InviteOp::Create(new) => client
            .create_invite(&new)
            .await
            .map(|_| tr("Ссылка создана").to_owned())
            .map_err(fail),
        InviteOp::Update(id, patch) => client
            .update_invite(&id, &patch)
            .await
            .map(|_| tr("Ссылка обновлена").to_owned())
            .map_err(fail),
        InviteOp::Revoke(id) => client
            .revoke_invite(&id)
            .await
            .map(|_| tr("Ссылка отозвана").to_owned())
            .map_err(fail),
        InviteOp::Reinstate(id, patch) => client
            .reinstate_invite(&id, &patch)
            .await
            .map(|v| {
                if v.get("tokenRotated").and_then(serde_json::Value::as_bool) == Some(true) {
                    tr("Ссылка восстановлена с новым адресом").to_owned()
                } else {
                    tr("Ссылка восстановлена").to_owned()
                }
            })
            .map_err(fail),
        InviteOp::Delete(id) => client
            .delete_invite(&id)
            .await
            .map(|_| tr("Ссылка удалена").to_owned())
            .map_err(fail),
        InviteOp::Redemptions(_) => Ok(String::new()),
    }
}

/// Выполняет действие с пиром.
pub(super) async fn peer_op(client: &bs_api::ApiClient, op: PeerOp) -> Result<String, String> {
    let fail = |err: bs_api::ApiError| upload_message(&err);
    match op {
        PeerOp::Recheck(id) => {
            let result = client.recheck_peer(&id).await.map_err(fail)?;
            Ok(if result.recovered {
                tr("Связь восстановлена").to_owned()
            } else {
                tr("Инстанс по-прежнему недоступен").to_owned()
            })
        }
        PeerOp::Rotate(id) => client
            .rotate_peer_secret(&id)
            .await
            .map(|_| tr("Ротация секрета начата").to_owned())
            .map_err(fail),
        PeerOp::Revoke(id) => client
            .revoke_peer(&id)
            .await
            .map(|_| tr("Пир отозван").to_owned())
            .map_err(fail),
        PeerOp::Reset(id) => client
            .reset_peer(&id)
            .await
            .map(|_| tr("Пир сброшен").to_owned())
            .map_err(fail),
        PeerOp::Delete(id) => client
            .delete_peer_permanently(&id)
            .await
            .map(|_| tr("Пир удалён").to_owned())
            .map_err(fail),
        PeerOp::Interval(id, days) => client
            .set_peer_interval(&id, days)
            .await
            .map(|_| tr("Интервал обновлён").to_owned())
            .map_err(fail),
        PeerOp::Initiate(origin) => client
            .initiate_peering(&origin)
            .await
            .map(|_| tr("Запрос на пиринг отправлен").to_owned())
            .map_err(fail),
        PeerOp::Answer(id, approve) => client
            .answer_approval(&id, approve)
            .await
            .map(|_| {
                if approve {
                    tr("Одобрено")
                } else {
                    tr("Отклонено")
                }
                .to_owned()
            })
            .map_err(fail),
    }
}

/// Выполняет запрос настроек.
pub(super) async fn handle_admin(client: &bs_api::ApiClient, request: AdminRequest) -> AdminEvent {
    let text = |err: &bs_api::ApiError| upload_message(err);
    match request {
        AdminRequest::Personal(op) => AdminEvent::Personal(personal_op(client, op).await),
        AdminRequest::Bots(op) => AdminEvent::Bots(bot_op(client, op).await),
        AdminRequest::LoadInvites { archived } => AdminEvent::Invites(
            client
                .invites(archived)
                .await
                .map(|list| (archived, list))
                .map_err(|e| text(&e)),
        ),
        AdminRequest::Invite(InviteOp::Redemptions(id)) => {
            let result = client.invite_redemptions(&id).await.map_err(|e| text(&e));
            AdminEvent::Redemptions { id, result }
        }
        AdminRequest::Invite(op) => AdminEvent::InviteDone(invite_op(client, op).await),
        AdminRequest::LoadPeers => AdminEvent::Peers(
            async {
                let peers = client.peers().await?;
                let requests = client.approval_requests().await.unwrap_or_default();
                Ok::<_, bs_api::ApiError>((peers, requests))
            }
            .await
            .map_err(|e| text(&e)),
        ),
        AdminRequest::Peer(op) => AdminEvent::PeerDone(peer_op(client, op).await),
        AdminRequest::LoadStreaming => {
            AdminEvent::Streaming(client.streaming_limits().await.map_err(|e| text(&e)))
        }
        AdminRequest::LoadVoiceBitrate => {
            AdminEvent::VoiceBitrate(client.instance_voice_bitrate().await.map_err(|e| text(&e)))
        }
        AdminRequest::SaveVoiceBitrate(kbps) => AdminEvent::VoiceBitrate(
            client
                .set_instance_voice_bitrate(kbps)
                .await
                .map_err(|e| text(&e)),
        ),
        AdminRequest::SaveStreaming(patch) => {
            AdminEvent::Streaming(client.update_streaming(&patch).await.map_err(|e| text(&e)))
        }
        AdminRequest::LoadTelemetry => {
            AdminEvent::Telemetry(client.telemetry().await.map_err(|e| text(&e)))
        }
        AdminRequest::SetTelemetry(on) => {
            AdminEvent::Telemetry(client.set_telemetry(on).await.map_err(|e| text(&e)))
        }
        AdminRequest::LoadTelemetryPreview => AdminEvent::TelemetryPreview(
            client
                .telemetry_preview()
                .await
                .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
                .map_err(|e| text(&e)),
        ),
        AdminRequest::LoadUpdates(refresh) => {
            AdminEvent::Updates(updates(client, refresh, false).await)
        }
        AdminRequest::UpdateServer => AdminEvent::Updates(updates(client, false, true).await),
        AdminRequest::LoadServer => AdminEvent::Server(server_state(client, None).await),
        AdminRequest::ServerJob { job, arg } => {
            AdminEvent::Server(server_state(client, Some((&job, &arg))).await)
        }
        AdminRequest::LoadSettings => {
            AdminEvent::Settings(client.instance_settings().await.map_err(|e| text(&e)))
        }
        AdminRequest::SaveSettings(patch) => AdminEvent::SettingsSaved(
            client
                .update_instance_settings(&patch)
                .await
                .map_err(|e| text(&e)),
        ),
        AdminRequest::LoadUsers {
            query,
            page,
            show_deleted,
        } => AdminEvent::Users(
            client
                .admin_users(&query, page, show_deleted)
                .await
                .map_err(|e| text(&e)),
        ),
        AdminRequest::LoadStorage => {
            AdminEvent::Storage(client.storage_stats().await.map_err(|e| text(&e)))
        }
        AdminRequest::TwoFactor(op) => AdminEvent::TwoFactor(two_factor(client, op).await),
        AdminRequest::Privacy(set) => AdminEvent::Privacy(privacy(client, set).await),
        AdminRequest::Passkeys(op) => AdminEvent::Passkeys(passkeys(client, op).await),
        AdminRequest::Cleanup { dry_run } => {
            AdminEvent::Cleaned(client.storage_cleanup(dry_run).await.map_err(|e| text(&e)))
        }
        other => match account_admin(client, other).await {
            Ok(event) | Err(event) => event,
        },
    }
}

/// Текст ошибки личного моста: подробность с той стороны, если есть.
fn personal_message(err: &bs_api::ApiError) -> String {
    if let bs_api::ApiError::Server { body, .. } = err {
        if let Some(message) = body
            .details
            .as_ref()
            .and_then(|d| d.get("message"))
            .and_then(serde_json::Value::as_str)
        {
            return message.to_owned();
        }
        match body.code.as_deref() {
            Some("personal_bridge_limit") => {
                return tr("Подключено слишком много аккаунтов").to_owned();
            }
            Some("personal_bridge_chat_limit") => {
                return tr("Связано слишком много чатов этого аккаунта").to_owned();
            }
            Some("personal_bridge_offline") => {
                return tr("Аккаунт ещё подключается — попробуйте через минуту").to_owned();
            }
            _ => {}
        }
    }
    err.to_string()
}

/// Текст ошибки действия с ботом.
fn bot_message(err: &bs_api::ApiError) -> String {
    if let bs_api::ApiError::Server { body, .. } = err {
        match body.code.as_deref() {
            Some("bot_username_invalid") => {
                return tr("Имя бота: 2–32 латинские буквы, цифры, «_», «.» или «-»").to_owned();
            }
            Some("bot_limit") => return tr("Ботов слишком много").to_owned(),
            Some("username_taken") => return tr("Это имя уже занято").to_owned(),
            Some("already_member") => return tr("Бот уже в этом пространстве").to_owned(),
            Some("user_banned") => return tr("Бот заблокирован в этом пространстве").to_owned(),
            _ => {}
        }
        if err.status() == Some(reqwest::StatusCode::FORBIDDEN) {
            return tr("Нужно право «Управление пространством»").to_owned();
        }
    }
    err.to_string()
}

/// Действие со своими ботами.
async fn bot_op(client: &bs_api::ApiClient, op: BotOp) -> BotEvent {
    let token = match op {
        BotOp::Load => {
            return BotEvent::Loaded(client.bots().await.map_err(|err| {
                (err.status() != Some(reqwest::StatusCode::NOT_FOUND)).then(|| bot_message(&err))
            }));
        }
        BotOp::Create {
            username,
            display_name,
            description,
        } => {
            let blank = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
            client
                .create_bot(
                    username.trim(),
                    blank(&display_name).as_deref(),
                    blank(&description).as_deref(),
                )
                .await
                .map(|made| Some((made.bot.id, made.token)))
        }
        BotOp::Update {
            id,
            display_name,
            description,
        } => {
            let patch = bs_api::BotPatch {
                display_name: Some(display_name),
                description: Some(description),
            };
            client.update_bot(&id, &patch).await.map(|_| None)
        }
        BotOp::Regenerate(id) => client
            .regenerate_bot_token(&id)
            .await
            .map(|token| Some((id, token))),
        BotOp::Delete(id) => client.delete_bot(&id).await.map(|()| None),
        BotOp::AddToSpace { bot, space } => {
            client.add_bot_to_space(&space, &bot).await.map(|()| None)
        }
    };
    match token {
        // Список мог не прочитаться, но новый токен терять нельзя.
        Ok(token) => BotEvent::Done {
            bots: client.bots().await.ok(),
            token,
        },
        Err(err) => BotEvent::Failed(bot_message(&err)),
    }
}

/// Свежий список личных мостов (`Err(None)` — сервер их не знает).
async fn personal_list(client: &bs_api::ApiClient) -> PersonalEvent {
    PersonalEvent::Loaded(client.personal_bridges().await.map_err(|err| {
        (err.status() != Some(reqwest::StatusCode::NOT_FOUND)).then(|| personal_message(&err))
    }))
}

/// Действие с личными мостами.
async fn personal_op(client: &bs_api::ApiClient, op: PersonalOp) -> PersonalEvent {
    let done = match op {
        PersonalOp::Load => return personal_list(client).await,
        PersonalOp::TelegramLogin { phone, app } => {
            return PersonalEvent::TelegramStarted(
                client
                    .telegram_login(&phone, app.as_ref().map(|(id, hash)| (*id, hash.as_str())))
                    .await
                    .map_err(|err| personal_message(&err)),
            );
        }
        PersonalOp::TelegramCode { login, code } => {
            return telegram_step(client.telegram_code(&login, &code).await);
        }
        PersonalOp::TelegramPassword { login, password } => {
            let step = client.telegram_password(&login, password.expose()).await;
            return telegram_step(step);
        }
        PersonalOp::RemoteChats(account) => {
            let result = client
                .personal_remote_chats(&account)
                .await
                .map_err(|err| personal_message(&err));
            return PersonalEvent::RemoteChats { account, result };
        }
        PersonalOp::AddMatrix { homeserver, login } => client
            .add_matrix_bridge(&homeserver, &login)
            .await
            .map(drop),
        PersonalOp::Remove(id) => client.remove_personal_bridge(&id).await.map(drop),
        PersonalOp::Link { account, chat } => {
            client.link_personal_chat(&account, &chat).await.map(drop)
        }
        PersonalOp::Unlink { account, chat } => {
            client.unlink_personal_chat(&account, &chat).await.map(drop)
        }
    };
    match done {
        Ok(()) => personal_list(client).await,
        Err(err) => PersonalEvent::Failed(personal_message(&err)),
    }
}

fn telegram_step(step: Result<bs_api::TelegramStep, bs_api::ApiError>) -> PersonalEvent {
    match step {
        Ok(step) if step.password_required => {
            PersonalEvent::Telegram(Ok(TelegramNext::Password(step.hint)))
        }
        // Список экран попросит следом: аккаунт уже заведён.
        Ok(_) => PersonalEvent::Telegram(Ok(TelegramNext::Done)),
        Err(err) => PersonalEvent::Telegram(Err(personal_message(&err))),
    }
}

/// Состояние обновлений; `start` — сначала попросить сервер обновиться.
async fn updates(
    client: &bs_api::ApiClient,
    refresh: bool,
    start: bool,
) -> Result<Box<bs_api::UpdateStatus>, String> {
    if start {
        client
            .request_server_update(None)
            .await
            .map_err(|e| upload_message(&e))?;
    }
    client
        .update_status(refresh)
        .await
        .map(Box::new)
        .map_err(|e| upload_message(&e))
}

/// Пересылка: цитата с подписью автора (без ссылки, если автор запретил
/// или его настройку узнать не удалось).
async fn forward(client: &bs_api::ApiClient, request: BridgeRequest) -> BridgeResponse {
    let BridgeRequest::Forward { target, dm, items } = request else {
        return BridgeResponse::Done;
    };
    // Приватность — по разу на автора; неизвестно — скрываем.
    let mut hides: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    let mut quotes = Vec::with_capacity(items.len());
    for item in &items {
        let hide = if let Some(hide) = hides.get(&item.author_id) {
            *hide
        } else {
            let hide = client
                .hides_forward_author(&item.author_id)
                .await
                .unwrap_or(true);
            hides.insert(item.author_id.clone(), hide);
            hide
        };
        quotes.push(bs_api::forward_as_markdown(
            &item.author_id,
            &item.author_name,
            hide,
            &item.content,
            &item.attachments,
        ));
    }
    // Пачка — в как можно меньше сообщений: сотня отправок упёрлась бы в
    // лимит частоты.
    for content in bs_api::pack_forwards(&quotes, bs_types::MAX_MESSAGE_LENGTH) {
        let body = bs_api::SendMessageBody {
            content,
            attachments: Vec::new(),
            reply_to_id: None,
        };
        let sent = if dm {
            client.send_dm_message(&target, &body).await.map(drop)
        } else {
            client.send_channel_message(&target, &body).await.map(drop)
        };
        if let Err(err) = sent {
            return BridgeResponse::Compose(ComposeEvent::SendFailed(upload_message(&err)));
        }
    }
    BridgeResponse::Done
}

/// Приватность пересылки: узнать (`None`) или задать.
async fn privacy(client: &bs_api::ApiClient, set: Option<bool>) -> Result<bool, String> {
    let result = match set {
        None => client.privacy_hide_forward().await,
        Some(hide) => client.set_privacy_hide_forward(hide).await.map(|()| hide),
    };
    result.map_err(|e| upload_message(&e))
}

/// Резервная копия ключа: действие, затем её состояние.
async fn e2ee_backup(
    client: &bs_api::ApiClient,
    op: super::E2eeBackupOp,
) -> Result<Option<bs_api::E2eeBackup>, String> {
    match op {
        super::E2eeBackupOp::Load => {}
        super::E2eeBackupOp::Save { data, public_key } => client
            .put_e2ee_backup(&data, &public_key)
            .await
            .map_err(|e| upload_message(&e))?,
        super::E2eeBackupOp::Delete => client
            .delete_e2ee_backup()
            .await
            .map_err(|e| upload_message(&e))?,
    }
    client.e2ee_backup().await.map_err(|e| upload_message(&e))
}

/// Загружает картинки и меняет свой профиль в пространстве.
async fn set_space_profile(
    client: &bs_api::ApiClient,
    space_id: &str,
    avatar: ImageChange,
    banner: ImageChange,
    status: Option<String>,
) -> Result<(), String> {
    async fn file(
        client: &bs_api::ApiClient,
        change: ImageChange,
    ) -> Result<Option<String>, String> {
        match change {
            ImageChange::Keep => Ok(None),
            ImageChange::Clear => Ok(Some(String::new())),
            ImageChange::Upload(name, bytes) => {
                let mime = crate::media::mime_for_name(&name);
                client
                    .upload_file(&name, mime, &bytes, |_, _| {})
                    .await
                    .map(|attachment| Some(attachment.filename))
                    .map_err(|err| upload_message(&err))
            }
        }
    }
    let patch = bs_api::SpaceProfilePatch {
        avatar: file(client, avatar).await?,
        banner: file(client, banner).await?,
        custom_status: status,
    };
    client
        .set_space_profile(space_id, &patch)
        .await
        .map(drop)
        .map_err(|err| {
            if err.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                crate::i18n::tr("Этот инстанс не поддерживает свой профиль в пространстве")
                    .to_owned()
            } else {
                upload_message(&err)
            }
        })
}

/// Страница галереи: поиск без текста с фильтром «есть файл» или «есть
/// ссылка», по 50 сообщений.
async fn gallery_page(
    client: &bs_api::ApiClient,
    channel_id: String,
    kind: ChannelKind,
    links: bool,
    offset: u32,
) -> GalleryPage {
    let query = bs_api::MessageSearchQuery {
        has: Some(if links { "link" } else { "file" }.to_owned()),
        offset: Some(offset),
        limit: Some(50),
        ..Default::default()
    };
    let entry =
        |id: String, user: &bs_types::User, created_at, content: Option<String>, attachments| {
            GalleryEntry {
                message_id: id,
                author: user
                    .display_name
                    .clone()
                    .unwrap_or_else(|| user.username.clone()),
                created_at,
                content: content.unwrap_or_default(),
                attachments,
            }
        };
    let result = match kind {
        ChannelKind::Space => client
            .search_channel_messages(&channel_id, &query)
            .await
            .map(|r| {
                let list = r
                    .results
                    .into_iter()
                    .map(|m| {
                        entry(
                            m.message.id,
                            &m.user,
                            m.message.created_at,
                            m.message.content,
                            m.attachments,
                        )
                    })
                    .collect();
                (list, r.total_count)
            }),
        ChannelKind::Dm => client
            .search_dm_messages(&channel_id, &query)
            .await
            .map(|r| {
                let list = r
                    .results
                    .into_iter()
                    .map(|m| {
                        entry(
                            m.message.id,
                            &m.user,
                            m.message.created_at,
                            m.message.content,
                            m.attachments,
                        )
                    })
                    .collect();
                (list, r.total_count)
            }),
    };
    GalleryPage {
        channel_id,
        links,
        offset,
        result: result.map_err(|err| space_message(&err)),
    }
}

/// Ключи доступа: добавление ждёт подтверждения в браузере до пяти минут.
async fn passkeys(
    client: &bs_api::ApiClient,
    op: super::PasskeyOp,
) -> Result<Vec<bs_api::Passkey>, String> {
    match op {
        super::PasskeyOp::List => {}
        super::PasskeyOp::Remove(id) => {
            client
                .delete_passkey(&id)
                .await
                .map_err(|e| upload_message(&e))?;
        }
        super::PasskeyOp::Add => {
            let ticket = client.passkey_register_ticket().await.map_err(|e| {
                if e.code() == Some("passkeys_unavailable") {
                    tr(
                        "Сервер не знает свой публичный адрес: администратору нужно задать \
                        PUBLIC_ORIGIN или DOMAIN в настройках сервера",
                    )
                    .to_owned()
                } else {
                    upload_message(&e)
                }
            })?;
            crate::sys::open_url(&ticket.url)?;
            let mut done = false;
            for _ in 0..150 {
                bs_rt::time::sleep(std::time::Duration::from_secs(2)).await;
                match client.passkey_status(&ticket.ticket).await {
                    Ok(bs_api::PasskeyStatus::Pending) => {}
                    Ok(bs_api::PasskeyStatus::Done { .. }) => {
                        done = true;
                        break;
                    }
                    Ok(bs_api::PasskeyStatus::Failed(reason)) => return Err(reason),
                    Err(e) => return Err(upload_message(&e)),
                }
            }
            if !done {
                return Err(crate::i18n::tr("Время вышло — ключ не добавлен").to_owned());
            }
        }
    }
    client.passkeys().await.map_err(|e| upload_message(&e))
}

/// Состояние сервера; `job` — сначала отдать заявку помощнику.
async fn server_state(
    client: &bs_api::ApiClient,
    job: Option<(&str, &str)>,
) -> Result<Box<bs_api::ServerState>, String> {
    if let Some((job, arg)) = job {
        client
            .server_job(job, arg)
            .await
            .map_err(|e| match e.code() {
                Some("helper_busy") => {
                    tr("Помощник занят — дождитесь окончания прошлой заявки").to_owned()
                }
                Some("updater_outdated") => {
                    tr("Помощник на сервере устарел — выполните на сервере sudo sh setup.sh")
                        .to_owned()
                }
                Some("updater_unavailable") => tr("Помощник на сервере не поставлен").to_owned(),
                _ => upload_message(&e),
            })?;
    }
    client
        .server_state()
        .await
        .map(Box::new)
        .map_err(|e| upload_message(&e))
}

/// Учётные записи: роли, сброс и смена пароля, удаление.
async fn account_admin(
    client: &bs_api::ApiClient,
    request: AdminRequest,
) -> Result<AdminEvent, AdminEvent> {
    let text = |err: &bs_api::ApiError| upload_message(err);
    let done =
        |action: AdminDone, result: Result<String, String>| AdminEvent::Done { action, result };
    Ok(match request {
        AdminRequest::SetRole { id, admin } => done(
            AdminDone::Role,
            client
                .admin_set_role(&id, admin)
                .await
                .map(|_| tr("Роль изменена").to_owned())
                .map_err(|e| text(&e)),
        ),
        AdminRequest::ResetPassword(id) => done(
            AdminDone::Reset,
            client
                .admin_reset_password(&id)
                .await
                .map(|r| r.temporary_password)
                .map_err(|e| text(&e)),
        ),
        AdminRequest::DeleteUser(id) => done(
            AdminDone::Delete,
            client
                .admin_delete_user(&id)
                .await
                .map(|_| tr("Пользователь удалён").to_owned())
                .map_err(|e| text(&e)),
        ),
        AdminRequest::ChangePassword { current, new } => {
            let body = current.map_or_else(
                || bs_api::ChangePasswordRequest::new(new.expose()),
                |current| {
                    bs_api::ChangePasswordRequest::with_current(current.expose(), new.expose())
                },
            );
            done(
                AdminDone::Password,
                client
                    .change_password(&body)
                    .await
                    .map(|_| tr("Пароль изменён").to_owned())
                    .map_err(|e| text(&e)),
            )
        }
        AdminRequest::DeleteAccount { password, username } => done(
            AdminDone::Account,
            client
                .delete_account(password.expose(), &username)
                .await
                .map(|_| tr("Аккаунт удалён").to_owned())
                .map_err(|e| text(&e)),
        ),
        _ => {
            return Err(AdminEvent::Done {
                action: AdminDone::Account,
                result: Err(tr("неизвестный запрос").to_owned()),
            });
        }
    })
}

/// Понятный текст отказа сервера для композера.
/// Личные стикеры и стикерпаки.
async fn sticker_request(client: &bs_api::ApiClient, request: BridgeRequest) -> BridgeResponse {
    match request {
        BridgeRequest::Stickers => BridgeResponse::Compose(ComposeEvent::Stickers(
            client.stickers().await.map_err(|err| upload_message(&err)),
        )),
        BridgeRequest::UploadSticker { name, bytes, pack } => {
            let result = match client.upload_sticker(&name, &bytes).await {
                Ok(sticker) => match pack {
                    Some(pack) => client
                        .put_pack_sticker(&pack, &sticker.id)
                        .await
                        .map(|_| ())
                        .map_err(|err| upload_message(&err)),
                    None => Ok(()),
                },
                Err(err) => Err(upload_message(&err)),
            };
            BridgeResponse::Compose(ComposeEvent::StickerChanged(result))
        }
        BridgeRequest::StickerPacks(op) => sticker_pack_op(client, op).await,
        BridgeRequest::RenameSticker { id, name } => {
            BridgeResponse::Compose(ComposeEvent::StickerChanged(
                client
                    .rename_sticker(&id, &name)
                    .await
                    .map_err(|err| upload_message(&err)),
            ))
        }
        BridgeRequest::DeleteSticker(id) => BridgeResponse::Compose(ComposeEvent::StickerChanged(
            client
                .delete_sticker(&id)
                .await
                .map(|_| ())
                .map_err(|err| upload_message(&err)),
        )),
        _ => BridgeResponse::Compose(ComposeEvent::StickerChanged(Ok(()))),
    }
}

/// Действие со стикерпаками, затем свежий список.
async fn sticker_pack_op(
    client: &bs_api::ApiClient,
    op: super::protocol::PackOp,
) -> BridgeResponse {
    use super::protocol::{PackError, PackOp};
    let done = match op {
        PackOp::With(sticker) => {
            return BridgeResponse::Compose(ComposeEvent::PacksWith(
                client
                    .sticker_packs_with(&sticker)
                    .await
                    .map_err(|err| upload_message(&err)),
            ));
        }
        PackOp::List => Ok(()),
        PackOp::Create(name) => client.create_sticker_pack(&name).await.map(drop),
        PackOp::Rename { pack, name } => client
            .update_sticker_pack(&pack, Some(&name), None)
            .await
            .map(drop),
        PackOp::Share { pack, shared } => client
            .update_sticker_pack(&pack, None, Some(shared))
            .await
            .map(drop),
        PackOp::Delete(pack) => client.delete_sticker_pack(&pack).await.map(drop),
        PackOp::Put { pack, sticker } => client.put_pack_sticker(&pack, &sticker).await.map(drop),
        PackOp::Remove { pack, sticker } => {
            client.remove_pack_sticker(&pack, &sticker).await.map(drop)
        }
        PackOp::Reorder { pack, order } => {
            client.reorder_sticker_pack(&pack, &order).await.map(drop)
        }
        PackOp::Added { pack, added } => {
            client.set_sticker_pack_added(&pack, added).await.map(drop)
        }
    };
    let result = match done {
        Err(err) => Err(PackError::Failed(upload_message(&err))),
        Ok(()) => client.sticker_packs().await.map_err(|err| {
            if err.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                PackError::Unsupported
            } else {
                PackError::Failed(upload_message(&err))
            }
        }),
    };
    BridgeResponse::Compose(ComposeEvent::StickerPacks(result))
}

pub(super) fn upload_message(err: &bs_api::ApiError) -> String {
    use bs_api::ApiError;
    match err {
        ApiError::Unexpected { status, .. } if status.as_u16() == 413 => {
            tr("Файл больше лимита сервера").to_owned()
        }
        ApiError::Unexpected { status, .. } if status.as_u16() == 429 => {
            tr("Слишком много загрузок, подождите минуту").to_owned()
        }
        ApiError::Unexpected { status, .. } | ApiError::Server { status, .. }
            if status.as_u16() == 403 =>
        {
            tr("Нет права прикреплять файлы").to_owned()
        }
        other => other.to_string(),
    }
}

/// Текст отказа сервера на действие над пространством.
pub(super) async fn group_admin(
    client: &bs_api::ApiClient,
    dm_id: String,
    op: GroupOp,
) -> BridgeResponse {
    let mut created = None;
    let result = match &op {
        GroupOp::Create(users) => client.create_group_dm(users).await.map(|dm| {
            created = Some(Box::new(dm));
        }),
        GroupOp::Rename(name) => client
            .update_group_dm(
                &dm_id,
                &bs_api::GroupUpdate {
                    name: Some(name.clone()),
                    icon: None,
                },
            )
            .await
            .map(|_| ()),
        GroupOp::Add(user) => client.add_group_member(&dm_id, user).await.map(|_| ()),
        GroupOp::Remove(user) => client.remove_group_member(&dm_id, user).await.map(|_| ()),
        GroupOp::Leave => client.leave_group_dm(&dm_id).await.map(|_| ()),
        GroupOp::Transfer(user) => client.transfer_group_owner(&dm_id, user).await.map(|_| ()),
        GroupOp::Icon(bytes) => match group_icon(client, &dm_id, bytes.as_deref()).await {
            Ok(()) => Ok(()),
            Err(message) => {
                let event = GroupEvent {
                    dm_id,
                    op,
                    result: Err(message),
                };
                return BridgeResponse::GroupAction(event, None);
            }
        },
    }
    .map_err(|err| space_message(&err));
    let dm_id = created.as_ref().map_or(dm_id, |dm| dm.id.clone());
    BridgeResponse::GroupAction(GroupEvent { dm_id, op, result }, created)
}

/// Сторона иконки группы после обрезки.
pub(super) const GROUP_ICON_SIDE: u32 = 512;

/// Обрезает картинку до квадрата, загружает и ставит иконкой группы;
/// `None` убирает иконку.
pub(super) async fn group_icon(
    client: &bs_api::ApiClient,
    dm_id: &str,
    bytes: Option<&[u8]>,
) -> Result<(), String> {
    let icon = match bytes {
        None => None,
        Some(bytes) => {
            let png = bs_media::square_icon(bytes, GROUP_ICON_SIDE, &bs_media::Limits::default())
                .map_err(|err| crate::tf!("Картинка не подходит: {err}", err = err))?;
            let attachment = client
                .upload_file("group-icon.png", "image/png", &png, |_, _| {})
                .await
                .map_err(|err| space_message(&err))?;
            Some(attachment.filename)
        }
    };
    client
        .update_group_dm(
            dm_id,
            &bs_api::GroupUpdate {
                name: None,
                icon: Some(icon),
            },
        )
        .await
        .map(|_| ())
        .map_err(|err| space_message(&err))
}

pub(super) fn space_message(err: &bs_api::ApiError) -> String {
    let message = err
        .body()
        .map_or_else(|| err.to_string(), |b| b.error.clone());
    crate::logic::describe_server_error(err.code(), &message)
}

/// Битрейт микрофона для звонка: у сервера Rustspace он свой, оригинал
/// его не знает (тогда `None` — битрейт LiveKit по умолчанию).
async fn voice_bitrate_if(
    client: &bs_api::ApiClient,
    target: &str,
    wanted: bool,
) -> Option<bs_api::VoiceBitrate> {
    if !wanted {
        return None;
    }
    client
        .voice_bitrate(target)
        .await
        .ok()
        .filter(|bitrate| bitrate.effective_kbps > 0)
}

/// Ограничения стриминга инстанса, если токен выдан (иначе звонка нет).
async fn streaming_limits_if(
    client: &bs_api::ApiClient,
    wanted: bool,
) -> Option<bs_api::StreamingLimits> {
    if !wanted {
        return None;
    }
    client
        .streaming_limits()
        .await
        .map_err(|err| tracing::debug!(%err, "ограничения стриминга недоступны"))
        .ok()
}

/// Токены и битрейт голоса. Чужие запросы возвращает обратно.
async fn voice_request(
    client: &bs_api::ApiClient,
    request: BridgeRequest,
) -> Result<BridgeResponse, BridgeRequest> {
    match request {
        BridgeRequest::StreamPreview(PreviewOp::Put { channel_id, jpeg }) => {
            // Кадр не дошёл — придёт следующий, сказать пользователю нечего.
            let _ = client.put_stream_preview(&channel_id, jpeg).await;
            Ok(BridgeResponse::Quiet)
        }
        BridgeRequest::StreamPreview(PreviewOp::Get {
            channel_id,
            user_id,
        }) => Ok(BridgeResponse::StreamPreview {
            jpeg: client
                .stream_preview(&channel_id, &user_id)
                .await
                .ok()
                .flatten(),
            channel_id,
            user_id,
        }),
        BridgeRequest::DmVoiceToken(dm_id) => {
            let result = client
                .livekit_token_dm(&dm_id)
                .await
                .map_err(|err| upload_message(&err));
            let limits = streaming_limits_if(client, result.is_ok()).await;
            let bitrate = voice_bitrate_if(client, &dm_id, result.is_ok()).await;
            Ok(BridgeResponse::VoiceToken(VoiceTokenEvent {
                channel_id: dm_id,
                result,
                limits,
                bitrate,
            }))
        }
        BridgeRequest::VoiceToken(channel_id) => {
            let result = client
                .livekit_token_channel(&channel_id)
                .await
                .map_err(|err| upload_message(&err));
            let limits = streaming_limits_if(client, result.is_ok()).await;
            let bitrate = voice_bitrate_if(client, &channel_id, result.is_ok()).await;
            Ok(BridgeResponse::VoiceToken(VoiceTokenEvent {
                channel_id,
                result,
                limits,
                bitrate,
            }))
        }
        BridgeRequest::Extras(op) => {
            Ok(BridgeResponse::Extras(super::extras::run(client, op).await))
        }
        BridgeRequest::SetVoiceBitrate { target, kbps } => {
            let result = client
                .set_voice_bitrate(&target, kbps)
                .await
                .map_err(|err| upload_message(&err));
            Ok(BridgeResponse::VoiceBitrate(target, result))
        }
        other => Err(other),
    }
}

pub(super) async fn handle_request(
    client: &bs_api::ApiClient,
    request: BridgeRequest,
) -> BridgeResponse {
    let request = match social_request(client, request).await {
        Ok(response) => return response,
        Err(request) => request,
    };
    let request = match spaces_request(client, request).await {
        Ok(response) => return response,
        Err(request) => request,
    };
    let request = match federation_request(client, request).await {
        Ok(response) => return response,
        Err(request) => request,
    };
    let request = match history_request(client, request).await {
        Ok(response) => return response,
        Err(request) => request,
    };
    let request = match voice_request(client, request).await {
        Ok(response) => return response,
        Err(request) => request,
    };
    match request {
        BridgeRequest::Admin(request) => BridgeResponse::Admin(handle_admin(client, request).await),
        BridgeRequest::GifEnabled => BridgeResponse::Compose(ComposeEvent::GifEnabled(
            client
                .gif_enabled()
                .await
                .map_err(|err| upload_message(&err)),
        )),
        BridgeRequest::Gifs { query, pos } => {
            let result = client
                .gifs(&query, 30, &pos)
                .await
                .map_err(|err| err.to_string());
            BridgeResponse::Compose(ComposeEvent::Gifs { query, pos, result })
        }
        request @ (BridgeRequest::Stickers
        | BridgeRequest::UploadSticker { .. }
        | BridgeRequest::DeleteSticker(_)
        | BridgeRequest::RenameSticker { .. }
        | BridgeRequest::StickerPacks(_)) => sticker_request(client, request).await,
        BridgeRequest::SendCards {
            channel_id,
            dm,
            content,
            card,
        } => {
            let cards = std::slice::from_ref(&card);
            let sent = if dm {
                client
                    .send_dm_cards(&channel_id, content.as_deref(), cards)
                    .await
            } else {
                client
                    .send_cards(&channel_id, content.as_deref(), cards)
                    .await
            };
            // Сервер без карточек (оригинал или старая сборка) отвечает 404
            // без кода, а переписка, которая уходит через федерацию или мост, —
            // `cards_unsupported`: тогда карточка уходит разметкой.
            let result = match sent {
                Err(bs_api::ApiError::Server { status, body })
                    if (status == reqwest::StatusCode::NOT_FOUND && body.code.is_none())
                        || body.code.as_deref() == Some("cards_unsupported") =>
                {
                    let body = bs_api::SendMessageBody {
                        content: bs_api::card_as_markdown(content.as_deref(), &card),
                        attachments: Vec::new(),
                        reply_to_id: None,
                    };
                    if dm {
                        client.send_dm_message(&channel_id, &body).await.map(drop)
                    } else {
                        client
                            .send_channel_message(&channel_id, &body)
                            .await
                            .map(drop)
                    }
                }
                other => other,
            };
            match result {
                Ok(()) => BridgeResponse::Done,
                Err(err) => BridgeResponse::Compose(ComposeEvent::SendFailed(upload_message(&err))),
            }
        }
        request @ BridgeRequest::Forward { .. } => forward(client, request).await,
        BridgeRequest::SendRest { channel_id, body } => {
            match client.send_channel_message(&channel_id, &body).await {
                Ok(_) => BridgeResponse::Done,
                Err(err) => BridgeResponse::Compose(ComposeEvent::SendFailed(upload_message(&err))),
            }
        }
        // Остальное разобрано выше, а картинки и загрузки идут
        // параллельно (см. `spawn_job`).
        _ => BridgeResponse::Done,
    }
}

/// Друзья, личные каналы, профили. Чужие запросы возвращает обратно.
async fn social_request(
    client: &bs_api::ApiClient,
    request: BridgeRequest,
) -> Result<BridgeResponse, BridgeRequest> {
    Ok(match request {
        BridgeRequest::Friends => match client.friends().await {
            Ok(friends) => BridgeResponse::Friends(friends),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        // Справочника может не быть (оригинальный сервер): тогда ключи
        // идут прямо в переписке, и отказ здесь — не ошибка.
        BridgeRequest::E2eePublish(key) => {
            let _ = client.publish_e2ee_key(&key).await;
            BridgeResponse::Quiet
        }
        BridgeRequest::E2eeBackup(op) => BridgeResponse::E2eeBackup(e2ee_backup(client, op).await),
        BridgeRequest::E2eePeerKey {
            dm_channel_id,
            user_id,
        } => match client.e2ee_key_of(&user_id).await {
            Ok(bs_api::E2eeKey {
                public_key: Some(public_key),
                ..
            }) => BridgeResponse::E2eePeerKey {
                dm_channel_id,
                public_key,
            },
            _ => BridgeResponse::Quiet,
        },
        BridgeRequest::NotificationSettings => match client.notification_settings().await {
            Ok(list) => BridgeResponse::NotificationSettings(list),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::FriendRequests => match client.friend_requests().await {
            Ok(requests) => BridgeResponse::FriendRequests(requests),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::SearchUsers(query) => match client.search_users(&query).await {
            Ok(users) => BridgeResponse::Users(users),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::DmChannels => match client.dm_channels().await {
            Ok(channels) => BridgeResponse::DmChannels(channels),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::OpenDm(user_id) => {
            let request = bs_api::CreateDmRequest::user(user_id);
            match client.open_dm(&request).await {
                Ok(channel) => BridgeResponse::DmChannel(Box::new(channel)),
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::SendFriendRequest(request) => {
            match client.send_friend_request(&request).await {
                Ok(_) => BridgeResponse::Done,
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::RespondFriendRequest(id, accept) => {
            let decision = if accept {
                bs_api::FriendRequestDecision::Accept
            } else {
                bs_api::FriendRequestDecision::Decline
            };
            match client.respond_friend_request(&id, decision).await {
                Ok(()) => BridgeResponse::Done,
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::CancelFriendRequest(id) => match client.cancel_friend_request(&id).await {
            Ok(()) => BridgeResponse::Done,
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::RemoveFriend(user_id) => match client.remove_friend(&user_id).await {
            Ok(()) => BridgeResponse::Done,
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::Profile(user_id) => {
            let user = client
                .user(&user_id)
                .await
                .map(Box::new)
                .map_err(|err| upload_message(&err));
            let mutuals = client.mutuals(&user_id).await.ok();
            BridgeResponse::UserProfile(ProfileEvent {
                user_id,
                user,
                mutuals,
            })
        }
        BridgeRequest::UpdateProfile(body) => match client.update_me(&body).await {
            Ok(user) => BridgeResponse::Profile(Box::new(user)),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        other => return Err(other),
    })
}

/// Новое пространство; значок — как в настройках: загрузка файла, затем
/// PATCH. Не загрузился значок — пространство всё равно создано.
async fn create_space(
    client: &bs_api::ApiClient,
    body: &bs_api::CreateSpaceRequest,
    icon: Option<super::protocol::SpaceIcon>,
) -> BridgeResponse {
    let space = match client.create_space(body).await {
        Ok(space) => space,
        Err(err) => return BridgeResponse::Failed(err.to_string()),
    };
    if let Some(icon) = icon {
        let mime = crate::media::mime_for_name(&icon.name);
        match client
            .upload_file(&icon.name, mime, &icon.bytes, |_, _| {})
            .await
        {
            Ok(file) => {
                let update = bs_api::SpaceUpdate {
                    icon: Some(file.filename),
                    ..Default::default()
                };
                if let Err(err) = client.update_space(&space.id, &update).await {
                    tracing::warn!(%err, "значок нового пространства не сохранился");
                }
            }
            Err(err) => tracing::warn!(%err, "значок нового пространства не загрузился"),
        }
    }
    full_space(client, &space.id).await
}

/// Пространства, приглашения, каналы, каталог. Чужие запросы возвращает обратно.
async fn spaces_request(
    client: &bs_api::ApiClient,
    request: BridgeRequest,
) -> Result<BridgeResponse, BridgeRequest> {
    Ok(match request {
        BridgeRequest::SpaceInvite(space_id) => {
            let result = client
                .space_invite_code(&space_id)
                .await
                .map_err(|err| space_message(&err));
            BridgeResponse::SpaceAction(SpaceEvent::Invite { space_id, result })
        }
        BridgeRequest::InviteFriend { friend_id, request } => {
            let result = client
                .send_space_invite(&request)
                .await
                .map(|_| ())
                .map_err(|err| space_message(&err));
            BridgeResponse::SpaceAction(SpaceEvent::InviteSent { friend_id, result })
        }
        BridgeRequest::JoinByCode(code) => {
            let joined = match client.join_by_code(&code).await {
                Ok(id) => client.space(&id).await,
                Err(err) => Err(err),
            };
            match joined {
                Ok(space) => BridgeResponse::Space(Box::new(space)),
                Err(err) => {
                    BridgeResponse::SpaceAction(SpaceEvent::JoinFailed(space_message(&err)))
                }
            }
        }
        BridgeRequest::LeaveSpace { space_id, user_id } => {
            let result = client
                .leave_space(&space_id, &user_id)
                .await
                .map_err(|err| space_message(&err));
            BridgeResponse::SpaceAction(SpaceEvent::Left { space_id, result })
        }
        BridgeRequest::SpaceAdmin {
            space_id,
            op:
                op @ (SpaceOp::ListBridges(_)
                | SpaceOp::AddBridge(..)
                | SpaceOp::RemoveBridge(_)
                | SpaceOp::ListWebhooks(_)
                | SpaceOp::CreateWebhook(..)
                | SpaceOp::DeleteWebhook(_)),
        } => bridge_op(client, space_id, op).await,
        BridgeRequest::SpaceAdmin { space_id, op } => space_admin(client, space_id, op).await,
        BridgeRequest::GroupAdmin { dm_id, op } => group_admin(client, dm_id, op).await,
        BridgeRequest::CreateChannel { space_id, channel } => {
            let result = client
                .create_channel(&space_id, &channel)
                .await
                .map_err(|err| space_message(&err));
            BridgeResponse::SpaceAction(SpaceEvent::Created(result))
        }
        BridgeRequest::CreateCategory { space_id, name } => {
            let result = client
                .create_category(&space_id, &name)
                .await
                .map_err(|err| space_message(&err));
            BridgeResponse::SpaceAction(SpaceEvent::Created(result))
        }
        // Сервер отвечает голым пространством без каналов и участников:
        // детали берём отдельным запросом, иначе в кэше оказалось бы пустое
        // пространство до переподключения.
        BridgeRequest::CreateSpace(body, icon) => create_space(client, &body, icon).await,
        BridgeRequest::Explore(query) => match client.explore_spaces(&query, 50, 0).await {
            Ok(page) => BridgeResponse::Explore(page),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        // Каталог не настроен или хаб недоступен — это не ошибка экрана,
        // а пустой раздел с пояснением.
        BridgeRequest::Directory(query) => {
            BridgeResponse::Directory(match client.directory(&query, 50, 0).await {
                Ok(feed) => Ok(feed.spaces),
                Err(err) => Err(err.to_string()),
            })
        }
        BridgeRequest::InstanceInfo => BridgeResponse::InstanceInfo(
            client
                .instance_info()
                .await
                .map(Box::new)
                .map_err(|err| err.to_string()),
        ),
        BridgeRequest::JoinSpace { id, by_request } => {
            if by_request {
                match client.request_join_space(&id, None).await {
                    Ok(()) => BridgeResponse::JoinRequested,
                    Err(err) => BridgeResponse::Failed(err.to_string()),
                }
            } else {
                match client.join_public_space(&id).await {
                    Ok(space) => full_space(client, &space.id).await,
                    Err(err) => BridgeResponse::Failed(err.to_string()),
                }
            }
        }
        other => return Err(other),
    })
}

/// Подключения других инстансов и реестр. Чужие запросы возвращает обратно.
async fn federation_request(
    client: &bs_api::ApiClient,
    request: BridgeRequest,
) -> Result<BridgeResponse, BridgeRequest> {
    Ok(match request {
        BridgeRequest::IssueCredential { origin, password } => {
            issue_credential(client, &origin, password.as_ref()).await
        }
        BridgeRequest::EnsurePeered(origin) => BridgeResponse::Peering {
            result: client
                .ensure_peered(&origin)
                .await
                .map(|answer| answer.peering_status)
                .map_err(|err| err.to_string()),
            origin,
        },
        BridgeRequest::MarkProvisioned(origin) => {
            let request = FederationCredentialRequest::marking_provisioned(origin);
            match client.federation_credential(&request).await {
                Ok(_) => BridgeResponse::Synced,
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::LoadRegistry => BridgeResponse::Registry(
            client
                .federation_registry()
                .await
                .map_err(|e| e.to_string()),
        ),
        BridgeRequest::PutRegistry {
            entries,
            updated_at,
        } => BridgeResponse::RegistrySaved(
            client
                .put_federation_registry(&entries, updated_at)
                .await
                .map_err(|e| e.to_string()),
        ),
        BridgeRequest::DeleteIdentity { origins, mode } => BridgeResponse::IdentityDeleted(
            client
                .delete_federation_identity(&origins, mode)
                .await
                .map_err(|e| e.to_string()),
        ),
        BridgeRequest::AdoptSecret(secret) => BridgeResponse::Adopted(
            client
                .change_password(&ChangePasswordRequest::new(secret.expose()))
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
        ),
        BridgeRequest::SyncInstances(entries) => {
            match client.set_replicated_instances(&entries).await {
                Ok(_) => BridgeResponse::Synced,
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        other => return Err(other),
    })
}

/// История, поиск, переход к сообщению. Чужие запросы возвращает обратно.
async fn history_request(
    client: &bs_api::ApiClient,
    request: BridgeRequest,
) -> Result<BridgeResponse, BridgeRequest> {
    Ok(match request {
        BridgeRequest::History {
            channel_id,
            kind,
            before,
        } => {
            let cursor = before.map(bs_api::PagingCursor::Before);
            let items = match kind {
                ChannelKind::Space => client
                    .messages(&channel_id, cursor, Some(HISTORY_PAGE))
                    .await
                    .map(|page| HistoryItems::Space(page.messages)),
                ChannelKind::Dm => client
                    .dm_messages(&channel_id, cursor, Some(HISTORY_PAGE))
                    .await
                    .map(|page| HistoryItems::Dm(page.messages)),
            };
            match items {
                Ok(items) => BridgeResponse::History { channel_id, items },
                Err(err) => BridgeResponse::HistoryFailed {
                    channel_id,
                    message: err.to_string(),
                },
            }
        }
        BridgeRequest::SpaceProfiles(space_id) => BridgeResponse::SpaceProfiles {
            result: client
                .space_profiles(&space_id)
                .await
                .map_err(|err| space_message(&err)),
            space_id,
        },
        BridgeRequest::SetSpaceProfile {
            space_id,
            avatar,
            banner,
            status,
        } => {
            let result = match set_space_profile(client, &space_id, avatar, banner, status).await {
                // Свежий список — сессия сразу увидит своё.
                Ok(()) => client
                    .space_profiles(&space_id)
                    .await
                    .map_err(|err| space_message(&err)),
                Err(err) => Err(err),
            };
            BridgeResponse::SpaceProfileSaved { space_id, result }
        }
        BridgeRequest::Gallery {
            channel_id,
            kind,
            links,
            offset,
        } => BridgeResponse::Gallery(gallery_page(client, channel_id, kind, links, offset).await),
        BridgeRequest::Search {
            channel_id,
            kind,
            query,
            seq,
        } => {
            let result = match kind {
                ChannelKind::Space => client
                    .search_channel_messages(&channel_id, &query)
                    .await
                    .map(|r| (hits_of_space(r.results), r.total_count)),
                ChannelKind::Dm => client
                    .search_dm_messages(&channel_id, &query)
                    .await
                    .map(|r| (hits_of_dm(r.results), r.total_count)),
            };
            BridgeResponse::SearchDone {
                seq,
                result: result.map_err(|err| space_message(&err)),
            }
        }
        BridgeRequest::Around {
            channel_id,
            kind,
            message_id,
        } => {
            let result = match kind {
                ChannelKind::Space => client
                    .messages_around(&channel_id, &message_id, Some(AROUND_PAGE))
                    .await
                    .map(HistoryItems::Space),
                ChannelKind::Dm => client
                    .dm_messages_around(&channel_id, &message_id, Some(AROUND_PAGE))
                    .await
                    .map(HistoryItems::Dm),
            };
            BridgeResponse::AroundDone {
                channel_id,
                message_id,
                result: result.map_err(|err| space_message(&err)),
            }
        }
        other => return Err(other),
    })
}
