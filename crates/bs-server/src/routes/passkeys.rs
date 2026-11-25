//! Ключи доступа (passkey) — Rustspace; у оригинала их нет.
//!
//! WebAuthn работает только в браузере, поэтому вход и добавление ключа идут
//! «билетом», как вход на телевизоре: клиент берёт билет, открывает в
//! браузере страницу инстанса, там пользователь подтверждает ключ (Bitwarden,
//! телефон по QR, Windows Hello, связка ключей), а клиент опрашивает билет.
//! На странице и в клиенте один и тот же код из шести цифр — чтобы не
//! подтвердить чужой вход.
//!
//! - `POST /api/rustspace/passkeys/register` (вход) → `{ticket, code, url}`;
//! - `POST /api/rustspace/passkeys/login` → `{ticket, code, url}`;
//! - `GET /rustspace/passkey/:ticket` — страница для браузера;
//! - `GET /api/rustspace/passkeys/options/:ticket` — параметры WebAuthn;
//! - `POST /api/rustspace/passkeys/finish/:ticket` — ответ аутентификатора;
//! - `GET /api/rustspace/passkeys/status/:ticket` → `{status, token?, user?}`;
//! - `GET /api/rustspace/passkeys`, `DELETE /api/rustspace/passkeys/:id`.
//!
//! Вход по ключу заменяет и пароль, и второй фактор: ключ — это устройство
//! пользователя плюс его отпечаток или PIN.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Html;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};

use super::users::{USER_COLUMNS, sanitize_user};
use crate::auth::{AuthUser, sign_token};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ClientIp, JsonBody};
use crate::params;
use crate::passkeys::{self, StoredPasskey};
use crate::ratelimit::Limit;
use crate::state::{AppState, now_ms};

/// Сколько живёт билет.
const TICKET_TTL: Duration = Duration::from_mins(5);
/// Не больше стольких билетов одновременно (защита памяти).
const MAX_TICKETS: usize = 2000;
/// Билетов на вход с одного адреса.
const TICKET_LIMIT: Limit = Limit {
    name: "passkey_ticket",
    max: 20,
    window: Duration::from_mins(10),
};

#[derive(Debug, Clone)]
enum Purpose {
    /// Добавить ключ к аккаунту.
    Register { user_id: String, username: String },
    /// Войти.
    Login,
}

#[derive(Debug, Clone)]
enum Status {
    Pending,
    /// Ключ добавлен.
    Registered,
    /// Вход выполнен: токен и пользователь (отдаются один раз).
    SignedIn(Value),
    Failed(String),
}

#[derive(Debug, Clone)]
struct Ticket {
    purpose: Purpose,
    challenge: [u8; 32],
    code: String,
    status: Status,
    created: Instant,
}

static TICKETS: LazyLock<Mutex<HashMap<String, Ticket>>> = LazyLock::new(Mutex::default);

fn tickets() -> std::sync::MutexGuard<'static, HashMap<String, Ticket>> {
    TICKETS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Сайт и id сайта WebAuthn: из `PUBLIC_ORIGIN`, а без него — из `DOMAIN`
/// (так настроены обычные установки за Caddy: `https://<домен>`).
fn site(state: &AppState) -> ApiResult<(String, String)> {
    let origin = state
        .config
        .public_origin
        .clone()
        .or_else(|| {
            let domain = state.config.domain.as_deref()?.trim().trim_end_matches('/');
            (!domain.is_empty() && domain != "localhost").then(|| format!("https://{domain}"))
        })
        .ok_or_else(|| ApiError::conflict("passkeys_unavailable"))?;
    let host = url::Url::parse(&origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .ok_or_else(|| ApiError::conflict("passkeys_unavailable"))?;
    Ok((origin.trim_end_matches('/').to_owned(), host))
}

fn new_ticket(state: &AppState, purpose: Purpose) -> ApiResult<Value> {
    let (origin, _) = site(state)?;
    let ticket = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let code = format!("{:06}", rand::random::<u32>() % 1_000_000);
    let mut all = tickets();
    all.retain(|_, t| t.created.elapsed() < TICKET_TTL);
    if all.len() >= MAX_TICKETS {
        return Err(ApiError::new(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
        ));
    }
    all.insert(
        ticket.clone(),
        Ticket {
            purpose,
            challenge: rand::random(),
            code: code.clone(),
            status: Status::Pending,
            created: Instant::now(),
        },
    );
    Ok(json!({
        "ticket": ticket,
        "code": code,
        "url": format!("{origin}/rustspace/passkey/{ticket}"),
    }))
}

fn live_ticket(ticket: &str) -> ApiResult<Ticket> {
    tickets()
        .get(ticket)
        .filter(|t| t.created.elapsed() < TICKET_TTL)
        .cloned()
        .ok_or_else(|| ApiError::not_found("ticket_not_found"))
}

fn set_status(ticket: &str, status: Status) {
    if let Some(t) = tickets().get_mut(ticket) {
        t.status = status;
    }
}

/// `POST /api/rustspace/passkeys/register`.
pub async fn register(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let username = user.username.clone();
    Ok(Json(new_ticket(
        &state,
        Purpose::Register {
            user_id: user.id,
            username,
        },
    )?))
}

/// `POST /api/rustspace/passkeys/login`.
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
) -> ApiResult<Json<Value>> {
    state.limiter.check(TICKET_LIMIT, ip)?;
    Ok(Json(new_ticket(&state, Purpose::Login)?))
}

/// `GET /api/rustspace/passkeys/options/:ticket` — параметры для браузера.
pub async fn options(
    State(state): State<AppState>,
    Path(ticket): Path<String>,
) -> ApiResult<Json<Value>> {
    let found = live_ticket(&ticket)?;
    let (_, rp_id) = site(&state)?;
    let challenge = URL_SAFE_NO_PAD.encode(found.challenge);
    let body = match found.purpose {
        Purpose::Register { user_id, username } => {
            let owner = user_id.clone();
            let existing: Vec<String> = state
                .run(move |tx, _| {
                    Ok(tx
                        .query(
                            "SELECT id FROM bs_passkeys WHERE user_id = ?",
                            params![owner],
                        )?
                        .iter()
                        .filter_map(|row| row.text("id").ok())
                        .collect())
                })
                .await?;
            json!({
                "mode": "create",
                "code": found.code,
                "publicKey": {
                    "challenge": challenge,
                    "rp": { "id": rp_id, "name": "Rustspace" },
                    "user": {
                        "id": URL_SAFE_NO_PAD.encode(user_id.as_bytes()),
                        "name": username,
                        "displayName": username,
                    },
                    "pubKeyCredParams": passkeys::ALGORITHMS
                        .iter()
                        .map(|alg| json!({ "type": "public-key", "alg": alg }))
                        .collect::<Vec<_>>(),
                    "authenticatorSelection": {
                        "residentKey": "required",
                        "userVerification": "preferred",
                    },
                    "excludeCredentials": existing
                        .iter()
                        .map(|id| json!({ "type": "public-key", "id": id }))
                        .collect::<Vec<_>>(),
                    "attestation": "none",
                    "timeout": 120_000,
                }
            })
        }
        Purpose::Login => json!({
            "mode": "get",
            "code": found.code,
            "publicKey": {
                "challenge": challenge,
                "rpId": rp_id,
                "userVerification": "preferred",
                "timeout": 120_000,
            }
        }),
    };
    Ok(Json(body))
}

/// `POST /api/rustspace/passkeys/finish/:ticket` — ответ аутентификатора.
pub async fn finish(
    State(state): State<AppState>,
    Path(ticket): Path<String>,
    JsonBody(credential): JsonBody,
) -> ApiResult<Json<Value>> {
    let found = live_ticket(&ticket)?;
    if !matches!(found.status, Status::Pending) {
        return Err(ApiError::conflict("ticket_used"));
    }
    let (origin, rp_id) = site(&state)?;
    let outcome = match found.purpose {
        Purpose::Register { user_id, .. } => {
            finish_register(
                &state,
                &credential,
                &found.challenge,
                &origin,
                &rp_id,
                user_id,
            )
            .await
        }
        Purpose::Login => {
            finish_login(&state, &credential, &found.challenge, &origin, &rp_id).await
        }
    };
    match outcome {
        Ok(status) => {
            set_status(&ticket, status);
            Ok(Json(json!({ "ok": true })))
        }
        Err(err) => {
            set_status(&ticket, Status::Failed(err));
            Err(ApiError::bad_request("passkey_rejected"))
        }
    }
}

async fn finish_register(
    state: &AppState,
    credential: &Value,
    challenge: &[u8],
    origin: &str,
    rp_id: &str,
    user_id: String,
) -> Result<Status, String> {
    let key = passkeys::verify_registration(credential, challenge, origin, rp_id)
        .map_err(|err| err.to_string())?;
    let name = credential["name"]
        .as_str()
        .map(|n| n.chars().take(64).collect::<String>())
        .unwrap_or_default();
    state
        .run(move |tx, _| {
            tx.execute(
                "INSERT INTO bs_passkeys (id, user_id, alg, public_key, sign_count, name, \
                 created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    key.id,
                    user_id,
                    key.alg,
                    key.public_key,
                    i64::from(key.sign_count),
                    name,
                    now_ms()
                ],
            )?;
            Ok(())
        })
        .await
        .map_err(|_| "ключ уже добавлен".to_owned())?;
    Ok(Status::Registered)
}

async fn finish_login(
    state: &AppState,
    credential: &Value,
    challenge: &[u8],
    origin: &str,
    rp_id: &str,
) -> Result<Status, String> {
    let id = passkeys::assertion_id(credential).ok_or("ответ повреждён")?;
    let stored = state
        .run(move |tx, _| {
            Ok(tx
                .query_one(
                    "SELECT user_id, alg, public_key, sign_count FROM bs_passkeys WHERE id = ?",
                    params![&id],
                )?
                .and_then(|row| {
                    Some((
                        id.clone(),
                        row.text("user_id").ok()?,
                        StoredPasskey {
                            alg: row.int("alg").ok()?,
                            public_key: row.text("public_key").ok()?,
                            sign_count: u32::try_from(row.int("sign_count").ok()?).unwrap_or(0),
                        },
                    ))
                }))
        })
        .await
        .map_err(|_| "база недоступна".to_owned())?
        .ok_or("этот ключ не привязан ни к одному аккаунту")?;
    let (key_id, user_id, key) = stored;
    let count = passkeys::verify_assertion(credential, &key, challenge, origin, rp_id)
        .map_err(|err| err.to_string())?;
    let owner = user_id.clone();
    let row = state
        .run(move |tx, _| {
            tx.execute(
                "UPDATE bs_passkeys SET sign_count = ?, last_used_at = ? WHERE id = ?",
                params![i64::from(count), now_ms(), key_id],
            )?;
            Ok(tx.query_one(
                &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"),
                params![owner],
            )?)
        })
        .await
        .map_err(|_| "база недоступна".to_owned())?
        .ok_or("аккаунт не найден")?;
    if row.opt_flag("is_deleted").ok().flatten() == Some(true) {
        return Err("аккаунт удалён".to_owned());
    }
    let username = row.text("username").map_err(|e| e.to_string())?;
    let token = sign_token(state, &user_id, &username).map_err(|e| e.code().to_owned())?;
    let user = sanitize_user(&row, true).map_err(|e| e.to_string())?;
    Ok(Status::SignedIn(json!({ "token": token, "user": user })))
}

/// `GET /api/rustspace/passkeys/status/:ticket` — опрос клиента. Токен
/// отдаётся один раз, после этого билет удаляется.
pub async fn status(Path(ticket): Path<String>) -> ApiResult<Json<Value>> {
    let found = live_ticket(&ticket)?;
    Ok(Json(match found.status {
        Status::Pending => json!({ "status": "pending" }),
        Status::Registered => {
            tickets().remove(&ticket);
            json!({ "status": "done" })
        }
        Status::SignedIn(session) => {
            tickets().remove(&ticket);
            json!({ "status": "done", "token": session["token"], "user": session["user"] })
        }
        Status::Failed(reason) => {
            tickets().remove(&ticket);
            json!({ "status": "failed", "error": reason })
        }
    }))
}

/// `GET /api/rustspace/passkeys` — свои ключи.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let keys = state
        .run(move |tx, _| {
            Ok(tx
                .query(
                    "SELECT id, name, created_at, last_used_at FROM bs_passkeys \
                     WHERE user_id = ? ORDER BY created_at",
                    params![user.id],
                )?
                .iter()
                .map(|row| {
                    json!({
                        "id": row.text("id").unwrap_or_default(),
                        "name": row.text("name").unwrap_or_default(),
                        "createdAt": row.int("created_at").unwrap_or_default(),
                        "lastUsedAt": row.opt_int("last_used_at").ok().flatten(),
                    })
                })
                .collect::<Vec<_>>())
        })
        .await?;
    Ok(Json(json!(keys)))
}

/// `DELETE /api/rustspace/passkeys/:id`.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            tx.execute(
                "DELETE FROM bs_passkeys WHERE id = ? AND user_id = ?",
                params![id, user.id],
            )?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// `GET /rustspace/passkey/:ticket` — страница, где браузер работает с
/// ключом. Билет скрипт берёт из адреса.
pub async fn page(Path(_ticket): Path<String>) -> Html<&'static str> {
    Html(PAGE)
}

/// `GET /rustspace/passkey.js` — скрипт страницы (отдельным файлом: CSP
/// сервера запрещает встроенные скрипты).
pub async fn script() -> impl axum::response::IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/javascript; charset=utf-8",
        )],
        SCRIPT,
    )
}

const PAGE: &str = r#"<!doctype html>
<html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Rustspace — ключ доступа</title>
<style>
body{margin:0;min-height:100vh;display:grid;place-items:center;background:#15121c;color:#ece8f4;
font:16px/1.5 system-ui,sans-serif}
main{background:#1f1a29;border:1px solid #2f2840;border-radius:16px;padding:32px;max-width:380px;text-align:center}
h1{font-size:20px;margin:0 0 8px}p{color:#a79fb8;margin:8px 0}
.code{font:600 28px ui-monospace,monospace;letter-spacing:6px;color:#fff;margin:16px 0}
button{background:#e86a3a;color:#fff;border:0;border-radius:10px;padding:12px 22px;font-size:16px;cursor:pointer}
button:disabled{opacity:.5}.ok{color:#5fd39a}.err{color:#f2708a}
</style></head><body><main>
<h1 id="title">Ключ доступа Rustspace</h1>
<p>Проверьте, что код совпадает с кодом в окне Rustspace (это не код двухфакторной аутентификации):</p>
<div class="code" id="code">······</div>
<button id="go" disabled>Продолжить</button>
<p id="note"></p>
</main><script src="/rustspace/passkey.js"></script></body></html>"#;

const SCRIPT: &str = r#"const T=location.pathname.split("/").pop();
const b2a=b=>btoa(String.fromCharCode(...new Uint8Array(b))).replace(/\+/g,"-").replace(/\//g,"_").replace(/=+$/,"");
const a2b=s=>Uint8Array.from(atob(s.replace(/-/g,"+").replace(/_/g,"/")),c=>c.charCodeAt(0));
const note=(t,c)=>{const n=document.getElementById("note");n.textContent=t;n.className=c||""};
let opts;
fetch("/api/rustspace/passkeys/options/"+T).then(r=>r.ok?r.json():Promise.reject()).then(o=>{
 opts=o;document.getElementById("code").textContent=o.code;
 document.getElementById("title").textContent=o.mode==="create"?"Добавить ключ доступа":"Войти по ключу доступа";
 document.getElementById("go").disabled=false;
}).catch(()=>note("Ссылка устарела. Начните заново в приложении.","err"));
document.getElementById("go").onclick=async()=>{
 const go=document.getElementById("go");go.disabled=true;note("Подтвердите ключ…");
 try{
  const pk=opts.publicKey;pk.challenge=a2b(pk.challenge);
  let cred;
  if(opts.mode==="create"){
   pk.user.id=a2b(pk.user.id);(pk.excludeCredentials||[]).forEach(c=>c.id=a2b(c.id));
   cred=await navigator.credentials.create({publicKey:pk});
  }else{cred=await navigator.credentials.get({publicKey:pk});}
  const r=cred.response,body={rawId:b2a(cred.rawId),response:{clientDataJSON:b2a(r.clientDataJSON)},
   name:navigator.userAgentData?.platform||navigator.platform||""};
  if(r.attestationObject)body.response.attestationObject=b2a(r.attestationObject);
  if(r.authenticatorData){body.response.authenticatorData=b2a(r.authenticatorData);body.response.signature=b2a(r.signature);}
  const res=await fetch("/api/rustspace/passkeys/finish/"+T,{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify(body)});
  if(!res.ok)throw new Error();
  note("Готово. Можно вернуться в Rustspace.","ok");
 }catch(e){note("Не получилось. Попробуйте ещё раз в приложении.","err");}
};
"#;
