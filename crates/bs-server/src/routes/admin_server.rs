//! Состояние сервера для администратора: нагрузка, диски, резервные копии и
//! заявки помощнику (`rustspace-update.sh`).
//!
//! Сервер работает без root, поэтому всё, что требует прав, — резервная
//! копия, другой каталог для копий, перенос файлов пользователей на другой
//! диск — делает помощник по заявке `job-request.json`, а сюда возвращает
//! ход (`job.json`) и список копий (`backups.json`). Сведения считаются
//! только по запросу: клиент спрашивает, пока открыт раздел, и сервер
//! ничего не держит фоном.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::params;
use crate::routes::invites::as_admin;
use crate::state::{AppState, now_ms};

type Outcome = Result<Response, ApiError>;

/// Список копий старше этого обновляется по заявке `scan`.
const LIST_STALE_MS: i64 = 10 * 60_000;
/// Опрос дисков дольше — ответ без них (зависший сетевой диск).
const DISK_TIMEOUT: Duration = Duration::from_secs(3);

/// Счётчики процессора считаются по разнице между опросами, поэтому
/// `System` живёт между запросами; пока раздел закрыт, его никто не трогает.
static SYSTEM: Mutex<Option<(sysinfo::System, Instant)>> = Mutex::new(None);
/// Опрос дисков уже идёт (или завис на сетевом диске).
static DISKS_BUSY: AtomicBool = AtomicBool::new(false);
/// Последний удачный список дисков — на случай зависшего опроса.
static LAST_DISKS: Mutex<Option<Vec<DiskInfo>>> = Mutex::new(None);

/// Файловые системы, которые не диски.
const PSEUDO_FS: [&str; 14] = [
    "tmpfs", "devtmpfs", "overlay", "squashfs", "proc", "sysfs", "cgroup", "cgroup2", "devpts",
    "mqueue", "autofs", "efivarfs", "ramfs", "nsfs",
];

#[derive(Debug, Clone)]
struct DiskInfo {
    mount: PathBuf,
    fs: String,
    name: String,
    total: u64,
    available: u64,
}

fn list_disks() -> Vec<DiskInfo> {
    sysinfo::Disks::new_with_refreshed_list()
        .list()
        .iter()
        .map(|d| DiskInfo {
            mount: d.mount_point().to_path_buf(),
            fs: d.file_system().to_string_lossy().into_owned(),
            name: d.name().to_string_lossy().into_owned(),
            total: d.total_space(),
            available: d.available_space(),
        })
        .filter(|d| d.total > 0 && !PSEUDO_FS.contains(&d.fs.as_str()))
        .collect()
}

/// Диски без зависания: сетевой диск может не ответить на `statvfs`.
async fn disks() -> Vec<DiskInfo> {
    let cached = || {
        LAST_DISKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_default()
    };
    if DISKS_BUSY.swap(true, Ordering::AcqRel) {
        return cached();
    }
    let task = tokio::task::spawn_blocking(|| {
        let list = list_disks();
        DISKS_BUSY.store(false, Ordering::Release);
        list
    });
    match tokio::time::timeout(DISK_TIMEOUT, task).await {
        Ok(Ok(list)) => {
            *LAST_DISKS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(list.clone());
            list
        }
        _ => cached(),
    }
}

/// Диск, на котором лежит путь: самая длинная подходящая точка монтирования.
fn disk_of<'a>(disks: &'a [DiskInfo], path: &Path) -> Option<&'a DiskInfo> {
    disks
        .iter()
        .filter(|d| path.starts_with(&d.mount))
        .max_by_key(|d| d.mount.as_os_str().len())
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    }
}

fn health() -> Value {
    let mut guard = SYSTEM
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fresh = guard.is_none();
    let (system, last) = guard.get_or_insert_with(|| (sysinfo::System::new(), Instant::now()));
    // Слишком частый опрос даёт шум: тогда — прежние значения.
    if fresh || last.elapsed() >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL {
        system.refresh_cpu_usage();
        system.refresh_memory();
        if let Ok(pid) = sysinfo::get_current_pid() {
            system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        }
        *last = Instant::now();
    }
    let process = sysinfo::get_current_pid()
        .ok()
        .and_then(|pid| system.process(pid));
    let load = sysinfo::System::load_average();
    json!({
        // Первый опрос процессора ещё без разницы — значения нет.
        "cpu": if fresh { Value::Null } else { json!(system.global_cpu_usage()) },
        "cores": system.cpus().len(),
        "load": if cfg!(windows) { Value::Null } else { json!([load.one, load.five, load.fifteen]) },
        "memoryTotal": system.total_memory(),
        "memoryUsed": system.used_memory(),
        "processMemory": process.map(sysinfo::Process::memory),
        "processCpu": if fresh { None } else { process.map(sysinfo::Process::cpu_usage) },
        "uptime": process.map(sysinfo::Process::run_time),
        "systemUptime": sysinfo::System::uptime(),
    })
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .filter(Value::is_object)
}

/// Каталог помощника, если он поставлен.
fn helper_dir(state: &AppState) -> Option<PathBuf> {
    state
        .config
        .updater_dir
        .clone()
        .filter(|dir| dir.join("enabled").exists())
}

/// Занят ли помощник: обновлением или заявкой.
fn helper_busy(dir: &Path) -> bool {
    let running = |file: &str| {
        read_json(&dir.join(file))
            .is_some_and(|s| matches!(s["state"].as_str(), Some("running" | "pending")))
    };
    dir.join("request.json").exists()
        || dir.join("job-request.json").exists()
        || running("status.json")
        || running("job.json")
}

fn write_job(dir: &Path, job: &str, arg: &str) -> Result<(), ApiError> {
    let request = json!({"job": job, "arg": arg, "at": now_ms()});
    let temporary = dir.join("job-request.json.tmp");
    std::fs::write(&temporary, request.to_string())
        .and_then(|()| std::fs::rename(&temporary, dir.join("job-request.json")))
        .map_err(|err| {
            let _ = std::fs::remove_file(&temporary);
            tracing::warn!(%err, "заявка помощнику не записана");
            ApiError::internal()
        })
}

/// Резервные копии: из `backups.json` помощника. Устаревший список
/// помощник пересчитает по заявке `scan`.
fn backups(state: &AppState) -> Value {
    let Some(dir) = helper_dir(state) else {
        return json!({"available": false});
    };
    let list = read_json(&dir.join("backups.json"));
    let job = read_json(&dir.join("job.json")).unwrap_or(Value::Null);
    let pending = dir.join("job-request.json").exists();
    let stale = list
        .as_ref()
        .and_then(|l| l["at"].as_i64())
        .is_none_or(|at| now_ms() - at > LIST_STALE_MS);
    // Список есть только у помощника, который понимает заявки.
    if list.is_some() && stale && !helper_busy(&dir) {
        let _ = write_job(&dir, "scan", "");
    }
    let Some(list) = list else {
        return json!({"available": true, "supported": false});
    };
    let items = list["items"].as_array().cloned().unwrap_or_default();
    let total: u64 = items.iter().filter_map(|i| i["size"].as_u64()).sum();
    json!({
        "available": true,
        "supported": true,
        "dir": list["dir"],
        "items": items,
        "total": total,
        "listedAt": list["at"],
        "job": job,
        "pending": pending,
    })
}

/// `GET /api/rustspace/admin/server` — нагрузка, диски, копии, хранилище.
pub async fn server_state(State(state): State<AppState>, user: AuthUser) -> Outcome {
    let postgres = state.config.database_url.is_some();
    let database_size = match as_admin(&state, &user, move |tx, _| {
        if postgres {
            let row = tx.query_one(
                "SELECT pg_database_size(current_database()) AS size",
                params![],
            )?;
            Ok::<_, ApiError>(row.map(|r| r.int("size")).transpose()?)
        } else {
            Ok(None)
        }
    })
    .await?
    {
        Ok(size) => size,
        Err(refusal) => return Ok(refusal),
    };
    let config = &state.config;
    let database_size = database_size
        .map(|s| u64::try_from(s).unwrap_or(0))
        .or_else(|| {
            (!postgres).then(|| {
                ["", "-wal"]
                    .iter()
                    .filter_map(|suffix| {
                        let mut path = config.db_path.clone().into_os_string();
                        path.push(suffix);
                        std::fs::metadata(path).ok()
                    })
                    .map(|m| m.len())
                    .sum()
            })
        });
    let backups = backups(&state);
    let uploads = absolute(&config.upload_dir);
    let mut places: Vec<(&str, PathBuf)> = vec![
        ("server", absolute(Path::new("."))),
        ("uploads", uploads.clone()),
    ];
    if !postgres {
        places.push(("database", absolute(&config.db_path)));
    }
    if let Some(dir) = backups["dir"].as_str() {
        places.push(("backups", PathBuf::from(dir)));
    }
    let disks = disks().await;
    let mut used: Vec<(PathBuf, Vec<&str>)> = Vec::new();
    for (role, path) in &places {
        if let Some(disk) = disk_of(&disks, path) {
            match used.iter_mut().find(|(mount, _)| *mount == disk.mount) {
                Some((_, roles)) => roles.push(role),
                None => used.push((disk.mount.clone(), vec![role])),
            }
        }
    }
    let disk_json = |d: &DiskInfo, roles: &[&str]| {
        json!({
            "mount": d.mount.to_string_lossy(),
            "fs": d.fs,
            "name": d.name,
            "total": d.total,
            "available": d.available,
            "roles": roles,
        })
    };
    let all: Vec<Value> = disks
        .iter()
        .map(|d| {
            let roles = used
                .iter()
                .find(|(mount, _)| *mount == d.mount)
                .map(|(_, r)| r.clone())
                .unwrap_or_default();
            disk_json(d, &roles)
        })
        .collect();
    Ok(Json(json!({
        "health": health(),
        "online": state.hub.online_users().len(),
        "database": {
            "kind": if postgres { "postgres" } else { "sqlite" },
            "size": database_size,
        },
        "uploadDir": uploads.to_string_lossy(),
        "disks": all,
        "backups": backups,
        "at": now_ms(),
    }))
    .into_response())
}

/// Тело заявки: что сделать и с чем.
#[derive(Debug, serde::Deserialize)]
pub struct JobRequest {
    job: String,
    #[serde(default)]
    arg: String,
}

/// Имя копии — как у помощника: простые символы, не с точки.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Каталог — абсолютный, без «..» и странных символов. Остальное помощник
/// проверит сам: ему доверять серверу нельзя.
fn safe_dir(path: &str) -> bool {
    path.starts_with('/')
        && path.len() > 1
        && path.len() <= 255
        && !path.ends_with('/')
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b' ' | b'-'))
        && path.split('/').all(|part| part != ".." && part != ".")
        && !path.contains("//")
}

/// `POST /api/rustspace/admin/server/job` `{job, arg}`: `backup`
/// (`arg: "files"` — и файлы пользователей), `delete` (имя копии),
/// `backup-dir` и `upload-dir` (каталог на нужном диске), `scan`.
pub async fn request_job(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<JobRequest>,
) -> Outcome {
    if let Err(refusal) = as_admin(&state, &user, |_, _| Ok::<_, ApiError>(())).await? {
        return Ok(refusal);
    }
    let Some(dir) = helper_dir(&state) else {
        return Err(ApiError::conflict("updater_unavailable"));
    };
    if !dir.join("backups.json").exists() {
        return Err(ApiError::conflict("updater_outdated"));
    }
    let arg = body.arg.trim();
    let valid = match body.job.as_str() {
        "backup" => arg.is_empty() || arg == "files",
        "delete" => safe_name(arg),
        "backup-dir" | "upload-dir" => safe_dir(arg),
        "scan" => true,
        _ => false,
    };
    if !valid {
        return Err(ApiError::bad_request("validation_failed"));
    }
    if helper_busy(&dir) {
        return Err(ApiError::conflict("helper_busy"));
    }
    write_job(&dir, &body.job, arg)?;
    tracing::info!(job = %body.job, arg, admin = %user.id, "заявка помощнику");
    Ok((axum::http::StatusCode::ACCEPTED, Json(backups(&state))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths() {
        assert!(safe_name("rustspace-20261118-101010-manual"));
        assert!(!safe_name("../etc"));
        assert!(!safe_name(".hidden"));
        assert!(!safe_name("a/b"));
        assert!(safe_dir("/mnt/nfs share"));
        assert!(!safe_dir("/mnt/../etc"));
        assert!(!safe_dir("relative"));
        assert!(!safe_dir("/mnt/"));
        assert!(!safe_dir("/mnt/$(id)"));
        assert!(!safe_dir("/mnt//x"));
    }

    #[test]
    fn disk_of_picks_longest_mount() {
        let disk = |mount: &str| DiskInfo {
            mount: PathBuf::from(mount),
            fs: "ext4".into(),
            name: String::new(),
            total: 1,
            available: 1,
        };
        let disks = [disk("/"), disk("/mnt/data"), disk("/mnt")];
        let found = disk_of(&disks, Path::new("/mnt/data/rustspace-uploads"));
        assert_eq!(
            found.map(|d| d.mount.clone()),
            Some(PathBuf::from("/mnt/data"))
        );
        let found = disk_of(&disks, Path::new("/var/lib/rustspace"));
        assert_eq!(found.map(|d| d.mount.clone()), Some(PathBuf::from("/")));
    }
}
