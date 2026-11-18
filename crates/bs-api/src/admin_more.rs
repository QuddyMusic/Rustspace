//! Остальные админские панели: стриминг, телеметрия, обновления.

use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Ограничения стриминга (`GET /api/settings/streaming`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StreamingLimits {
    /// Максимальный битрейт, кбит/с.
    pub max_bitrate_kbps: u32,
    /// Минимальный битрейт, кбит/с.
    pub min_bitrate_kbps: u32,
    /// Шаг битрейта, кбит/с.
    pub bitrate_step_kbps: u32,
    /// Допустимые разрешения: числа и `"native"`.
    pub allowed_resolutions: Vec<serde_json::Value>,
    /// Допустимая частота кадров.
    pub allowed_framerates: Vec<u32>,
    /// Предельное разрешение.
    pub max_resolution: u32,
    /// Предельная частота кадров.
    pub max_framerate: u32,
    /// Разрешён ли свой битрейт.
    pub allow_custom_bitrate: bool,
    /// Свои битрейты администратора для ячеек «высота_частота», кбит/с.
    pub bitrate_matrix_overrides: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

/// Изменение ограничений: уходят только заданные поля.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamingPatch {
    /// Максимальный битрейт.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bitrate_kbps: Option<u32>,
    /// Минимальный битрейт.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_bitrate_kbps: Option<u32>,
    /// Шаг битрейта.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate_step_kbps: Option<u32>,
    /// Допустимые разрешения.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_resolutions: Option<Vec<serde_json::Value>>,
    /// Допустимая частота кадров.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_framerates: Option<Vec<u32>>,
    /// Предельное разрешение.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_resolution: Option<u32>,
    /// Предельная частота кадров.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_framerate: Option<u32>,
    /// Разрешён ли свой битрейт.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_custom_bitrate: Option<bool>,
}

/// Телеметрия (`GET /api/admin/telemetry`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TelemetryStatus {
    /// `None` — ещё не спрашивали.
    pub enabled: Option<bool>,
    /// Последний отправленный день.
    pub last_day: Option<String>,
    /// Случайный идентификатор.
    pub id: Option<String>,
}

/// Последний релиз.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LatestRelease {
    /// Версия.
    pub version: String,
    /// Ссылка на релиз.
    pub url: String,
    /// Дата публикации (ISO 8601).
    pub published_at: String,
}

/// Версия установленного.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CurrentVersion {
    /// Версия.
    pub version: String,
    /// Короткий хэш сборки.
    pub commit: Option<String>,
}

/// Состояние обновлений.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateStatus {
    /// Что установлено.
    pub current: CurrentVersion,
    /// Что вышло последним.
    pub latest: Option<LatestRelease>,
    /// `up-to-date`, `update-available` или `unknown`.
    pub state: String,
    /// Когда проверяли (мс).
    pub checked_at: Option<i64>,
    /// Включена ли проверка.
    pub check_enabled: bool,
    /// Почему состояние неизвестно.
    pub reason: Option<String>,
    /// Как установлен инстанс: `prebuilt`, `source`, `unknown`.
    pub channel: String,
    /// Обновление из клиента (`bs-server` с помощником из `setup.sh`).
    pub self_update: SelfUpdate,
}

/// Помощник обновления сервера и его последний шаг.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SelfUpdate {
    /// Помощник поставлен.
    pub available: bool,
    /// `idle`, `pending`, `running`, `done`, `failed`, `rolled-back`.
    pub state: String,
    /// Какую версию ставили.
    pub target: String,
    /// С какой.
    pub from: String,
    /// Шаг или причина (код).
    pub step: String,
    /// Где резервная копия.
    pub backup: String,
}

/// Состояние сервера для администратора (`GET /api/rustspace/admin/server`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerState {
    /// Нагрузка.
    pub health: ServerHealth,
    /// Сколько пользователей в сети.
    pub online: u64,
    /// База данных.
    pub database: DatabaseInfo,
    /// Где лежат файлы пользователей.
    pub upload_dir: String,
    /// Диски (точки монтирования).
    pub disks: Vec<DiskInfo>,
    /// Резервные копии.
    pub backups: Backups,
    /// Когда собраны сведения (мс).
    pub at: i64,
}

/// Нагрузка на сервер.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerHealth {
    /// Загрузка процессора, % (нет у первого опроса).
    pub cpu: Option<f32>,
    /// Ядер.
    pub cores: u32,
    /// Средняя загрузка за 1, 5 и 15 минут (не у Windows).
    pub load: Option<[f64; 3]>,
    /// Память всего, байт.
    pub memory_total: u64,
    /// Занято, байт.
    pub memory_used: u64,
    /// Память процесса сервера, байт.
    pub process_memory: Option<u64>,
    /// Процессор процесса сервера, % одного ядра.
    pub process_cpu: Option<f32>,
    /// Сколько работает сервер, с.
    pub uptime: Option<u64>,
    /// Сколько работает система, с.
    pub system_uptime: u64,
}

/// База данных.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DatabaseInfo {
    /// `sqlite` или `postgres`.
    pub kind: String,
    /// Размер, байт.
    pub size: Option<u64>,
}

/// Диск сервера.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiskInfo {
    /// Точка монтирования.
    pub mount: String,
    /// Файловая система (`ext4`, `nfs4`, `cifs`, `fuse.davfs`…).
    pub fs: String,
    /// Устройство или источник.
    pub name: String,
    /// Всего, байт.
    pub total: u64,
    /// Свободно, байт.
    pub available: u64,
    /// Что здесь лежит: `server`, `database`, `uploads`, `backups`.
    pub roles: Vec<String>,
}

/// Резервные копии (их ведёт помощник на сервере).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Backups {
    /// Помощник поставлен.
    pub available: bool,
    /// Помощник умеет заявки (иначе его надо обновить через `setup.sh`).
    pub supported: bool,
    /// Каталог копий.
    pub dir: String,
    /// Копии, новые сверху.
    pub items: Vec<BackupItem>,
    /// Всего занято, байт.
    pub total: u64,
    /// Последняя заявка и её ход.
    pub job: Option<HelperJob>,
    /// Заявка ждёт помощника.
    pub pending: bool,
}

/// Резервная копия.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BackupItem {
    /// Имя каталога копии.
    pub name: String,
    /// Размер, байт.
    pub size: u64,
    /// Когда сделана (мс).
    pub at: i64,
}

/// Заявка помощнику и её ход.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HelperJob {
    /// `backup`, `delete`, `backup-dir`, `upload-dir`, `scan`.
    pub job: String,
    /// `running`, `done`, `failed`.
    pub state: String,
    /// Шаг или причина (код).
    pub step: String,
    /// Когда (мс).
    pub at: i64,
}

#[derive(Serialize)]
struct Enabled {
    enabled: bool,
}

impl ApiClient {
    /// `GET /api/settings/streaming`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn streaming_limits(&self) -> Result<StreamingLimits, ApiError> {
        self.get_json("/api/settings/streaming").await
    }

    /// `PATCH /api/settings/streaming` — только администратор.
    ///
    /// # Errors
    /// Ошибки запроса и проверки значений.
    pub async fn update_streaming(
        &self,
        patch: &StreamingPatch,
    ) -> Result<StreamingLimits, ApiError> {
        self.patch_json("/api/settings/streaming", patch).await
    }

    /// `GET /api/admin/telemetry`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn telemetry(&self) -> Result<TelemetryStatus, ApiError> {
        self.get_json("/api/admin/telemetry").await
    }

    /// `PUT /api/admin/telemetry` — включить или выключить.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn set_telemetry(&self, enabled: bool) -> Result<TelemetryStatus, ApiError> {
        self.put_json("/api/admin/telemetry", &Enabled { enabled })
            .await
    }

    /// `GET /api/admin/telemetry/preview` — что именно отправится.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn telemetry_preview(&self) -> Result<serde_json::Value, ApiError> {
        self.get_json("/api/admin/telemetry/preview").await
    }

    /// `POST /api/rustspace/admin/update` — заявка обновить сервер до
    /// `version` (по умолчанию — последний выпуск).
    ///
    /// # Errors
    /// Ошибки запроса (409 — помощника нет или обновление уже идёт).
    pub async fn request_server_update(
        &self,
        version: Option<&str>,
    ) -> Result<SelfUpdate, ApiError> {
        self.post_json(
            "/api/rustspace/admin/update",
            &serde_json::json!({ "version": version }),
        )
        .await
    }

    /// `GET /api/rustspace/admin/server` — нагрузка, диски и резервные копии.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn server_state(&self) -> Result<ServerState, ApiError> {
        self.get_json("/api/rustspace/admin/server").await
    }

    /// `POST /api/rustspace/admin/server/job` — заявка помощнику: `backup`
    /// (`arg` `files` — и файлы), `delete`, `backup-dir`, `upload-dir`, `scan`.
    ///
    /// # Errors
    /// Ошибки запроса (409 — помощник занят, не поставлен или устарел).
    pub async fn server_job(&self, job: &str, arg: &str) -> Result<Backups, ApiError> {
        self.post_json(
            "/api/rustspace/admin/server/job",
            &serde_json::json!({ "job": job, "arg": arg }),
        )
        .await
    }

    /// `GET /api/admin/instance/update-status`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn update_status(&self, refresh: bool) -> Result<UpdateStatus, ApiError> {
        let path = if refresh {
            "/api/admin/instance/update-status?refresh=true"
        } else {
            "/api/admin/instance/update-status"
        };
        self.get_json(path).await
    }
}

#[cfg(test)]
mod tests {
    use super::{StreamingPatch, UpdateStatus};

    #[test]
    fn patch_sends_only_set_fields() {
        let patch = StreamingPatch {
            max_framerate: Some(60),
            ..StreamingPatch::default()
        };
        assert_eq!(
            serde_json::to_string(&patch).unwrap_or_default(),
            r#"{"maxFramerate":60}"#
        );
    }

    #[test]
    fn update_status_parses() {
        let parsed: UpdateStatus = serde_json::from_str(
            r#"{"current":{"version":"1.2.3","commit":null},"latest":null,"state":"unknown",
                "checkedAt":null,"checkEnabled":false,"reason":"disabled","channel":"source"}"#,
        )
        .unwrap_or_default();
        assert_eq!(parsed.current.version, "1.2.3");
        assert_eq!(parsed.reason.as_deref(), Some("disabled"));
    }
}
