//! Перенос истории чатов личных мостов.
//!
//! История переносится страницами от новых сообщений к старым, по одной
//! странице каждого ожидающего чата по очереди: большой канал не задерживает
//! остальные чаты. Где остановились, хранится в базе, так что после
//! перезапуска перенос продолжается. Старые сообщения пишутся молча, а
//! владельцу уходит `rs_dm_history`: клиент перечитывает ленту чата.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::db::DbResult;
use crate::params;
use crate::state::AppState;

/// Больше сообщений одного чата не переносим (каналы бывают огромными).
const MAX_IMPORT: i64 = 100_000;
/// После стольких ошибок подряд чат бросаем (например, из него вышли).
const MAX_FAILURES: i64 = 5;
/// Пауза после ошибки страницы.
const BACKOFF: Duration = Duration::from_secs(30);
/// Как часто без пинка проверять, не появилось ли работы.
const IDLE: Duration = Duration::from_mins(5);

/// Будит переносчиков: появился новый чат для переноса.
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Страница истории той стороны.
pub(super) struct HistoryPage {
    /// Ещё не перенесённые сообщения, от старых к новым.
    pub(super) messages: Vec<super::Remote>,
    /// Номер самого старого сообщения страницы (для следующей).
    pub(super) oldest: Option<String>,
    /// Старше сообщений нет.
    pub(super) end: bool,
}

/// Ставит чат в очередь переноса; `restart` — пройти историю заново
/// (уже перенесённое пропускается).
pub(super) async fn enqueue(state: &AppState, account: &str, chat: &str, restart: bool) {
    let (account, chat) = (account.to_owned(), chat.to_owned());
    let result = state
        .run(move |tx, _| {
            if restart {
                tx.execute(
                    "DELETE FROM bs_personal_imports WHERE account_id = ? AND remote_chat = ?",
                    params![&account, &chat],
                )?;
            }
            tx.execute(
                "INSERT INTO bs_personal_imports (account_id, remote_chat, oldest, imported, failures, done) \
                 VALUES (?, ?, NULL, 0, 0, 0) ON CONFLICT (account_id, remote_chat) DO NOTHING",
                params![&account, &chat],
            )?;
            Ok(())
        })
        .await;
    if let Err(err) = result {
        tracing::warn!(?err, "личный мост: не удалось поставить чат в перенос");
    }
    WAKE.notify_waiters();
}

/// Строка очереди.
struct Job {
    chat: String,
    oldest: Option<String>,
    imported: i64,
    failures: i64,
}

async fn pending(state: &AppState, account: &str) -> Vec<Job> {
    let account = account.to_owned();
    state
        .run(move |tx, _| {
            tx.query(
                "SELECT remote_chat, oldest, imported, failures FROM bs_personal_imports \
                 WHERE account_id = ? AND done = 0 ORDER BY remote_chat",
                params![&account],
            )?
            .iter()
            .map(|row| {
                Ok(Job {
                    chat: row.text("remote_chat")?,
                    oldest: row.opt_text("oldest")?,
                    imported: row.int("imported")?,
                    failures: row.int("failures")?,
                })
            })
            .collect::<DbResult<Vec<_>>>()
            .map_err(Into::into)
        })
        .await
        .unwrap_or_default()
}

/// Номера уже перенесённых сообщений чата.
async fn known(state: &AppState, account: &str, chat: &str) -> HashSet<String> {
    let (account, chat) = (account.to_owned(), chat.to_owned());
    state
        .run(move |tx, _| {
            tx.query(
                "SELECT remote_id FROM bs_personal_messages WHERE account_id = ? AND remote_chat = ?",
                params![&account, &chat],
            )?
            .iter()
            .map(|row| row.text("remote_id"))
            .collect::<DbResult<HashSet<_>>>()
            .map_err(Into::into)
        })
        .await
        .unwrap_or_default()
}

/// Переносчик аккаунта: живёт, пока подключён этот самый мост.
pub(super) fn start(state: &AppState, account: &str) {
    let (app, account) = (state.clone(), account.to_owned());
    let Some(link) = app.personal.link(&account) else {
        return;
    };
    tokio::spawn(async move {
        // Чаты, связанные до появления очереди, тоже переносятся целиком.
        let lookup = account.clone();
        let _ = app
            .run(move |tx, _| {
                tx.execute(
                    "INSERT INTO bs_personal_imports (account_id, remote_chat, oldest, imported, failures, done) \
                     SELECT account_id, remote_chat, NULL, 0, 0, 0 FROM bs_personal_chats \
                     WHERE account_id = ? ON CONFLICT (account_id, remote_chat) DO NOTHING",
                    params![&lookup],
                )?;
                Ok(())
            })
            .await;
        let mut seen: HashMap<String, HashSet<String>> = HashMap::new();
        loop {
            let jobs = pending(&app, &account).await;
            if jobs.is_empty() {
                seen.clear();
                let _ = tokio::time::timeout(IDLE, WAKE.notified()).await;
            }
            for job in jobs {
                if !app
                    .personal
                    .link(&account)
                    .is_some_and(|now| Arc::ptr_eq(&now, &link))
                {
                    return;
                }
                if !seen.contains_key(&job.chat) {
                    let ids = known(&app, &account, &job.chat).await;
                    seen.insert(job.chat.clone(), ids);
                }
                let skip = seen.get(&job.chat).cloned().unwrap_or_default();
                let page = link
                    .history_page(&account, &job.chat, job.oldest.as_deref(), &skip)
                    .await;
                step(&app, &account, job, page, &mut seen).await;
            }
            if !app
                .personal
                .link(&account)
                .is_some_and(|now| Arc::ptr_eq(&now, &link))
            {
                return;
            }
        }
    });
}

/// Пишет страницу и двигает очередь.
async fn step(
    app: &AppState,
    account: &str,
    job: Job,
    page: Result<HistoryPage, String>,
    seen: &mut HashMap<String, HashSet<String>>,
) {
    let page = match page {
        Ok(page) => page,
        Err(err) => {
            tracing::warn!(account, chat = job.chat, %err, "личный мост: страница истории не загрузилась");
            let failures = job.failures + 1;
            save(
                app,
                account,
                &job.chat,
                job.oldest,
                job.imported,
                failures,
                failures >= MAX_FAILURES,
            )
            .await;
            tokio::time::sleep(BACKOFF).await;
            return;
        }
    };
    let count = i64::try_from(page.messages.len()).unwrap_or(0);
    for message in page.messages {
        if let (Some(id), Some(ids)) = (&message.id, seen.get_mut(&job.chat)) {
            ids.insert(id.clone());
        }
        super::sync::deliver_quiet(app, message).await;
    }
    let imported = job.imported + count;
    let done = page.end || page.oldest.is_none() || imported >= MAX_IMPORT;
    save(app, account, &job.chat, page.oldest, imported, 0, done).await;
    if count > 0 {
        super::sync::fix_replies(app, account, &job.chat).await;
        announce(app, account, &job.chat).await;
    }
    if done {
        seen.remove(&job.chat);
        tracing::info!(
            account,
            chat = job.chat,
            imported,
            "личный мост: история перенесена"
        );
    }
}

async fn save(
    app: &AppState,
    account: &str,
    chat: &str,
    oldest: Option<String>,
    imported: i64,
    failures: i64,
    done: bool,
) {
    let (account, chat) = (account.to_owned(), chat.to_owned());
    let result = app
        .run(move |tx, _| {
            tx.execute(
                "UPDATE bs_personal_imports SET oldest = ?, imported = ?, failures = ?, done = ? \
                 WHERE account_id = ? AND remote_chat = ?",
                params![oldest, imported, failures, i64::from(done), &account, &chat],
            )?;
            Ok(())
        })
        .await;
    if let Err(err) = result {
        tracing::warn!(?err, "личный мост: не удалось сохранить ход переноса");
    }
}

/// Владельцу: в ленте чата появились старые сообщения.
async fn announce(app: &AppState, account: &str, chat: &str) {
    let (account, chat) = (account.to_owned(), chat.to_owned());
    let _ = app
        .run(move |tx, out| {
            let Some(row) = tx.query_one(
                "SELECT c.dm_channel_id, a.user_id FROM bs_personal_chats c \
                 JOIN bs_personal_accounts a ON a.id = c.account_id \
                 WHERE c.account_id = ? AND c.remote_chat = ?",
                params![&account, &chat],
            )?
            else {
                return Ok(());
            };
            out.to_user(
                &row.text("user_id")?,
                &json!({"type": "rs_dm_history", "channelId": row.text("dm_channel_id")?}),
            );
            Ok(())
        })
        .await;
}

/// Чат больше не переносится (удалён или отвязан).
pub(super) fn forget(
    tx: &mut dyn crate::db::Tx,
    account: &str,
    chat: Option<&str>,
) -> DbResult<()> {
    match chat {
        Some(chat) => {
            tx.execute(
                "DELETE FROM bs_personal_imports WHERE account_id = ? AND remote_chat = ?",
                params![account, chat],
            )?;
            tx.execute(
                "DELETE FROM bs_personal_replies WHERE account_id = ? AND remote_chat = ?",
                params![account, chat],
            )?;
        }
        None => {
            tx.execute(
                "DELETE FROM bs_personal_imports WHERE account_id = ?",
                params![account],
            )?;
            tx.execute(
                "DELETE FROM bs_personal_replies WHERE account_id = ?",
                params![account],
            )?;
        }
    }
    Ok(())
}
