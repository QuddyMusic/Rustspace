use thiserror::Error;

/// Ошибки голосового слоя.
#[derive(Debug, Error)]
pub enum VoiceError {
    #[error("livekit: {0}")]
    LiveKit(#[from] livekit::RoomError),

    #[error("audio: {0}")]
    Audio(#[from] livekit::AudioError),

    #[error("не удалось подключиться к комнате: {0}")]
    Connect(String),

    #[error("микрофон не найден в системе")]
    NoMicrophone,

    #[error("микрофон не опубликован")]
    MicNotPublished,

    #[error("камера не опубликована")]
    CameraNotPublished,

    #[error("демонстрация экрана не запущена")]
    ScreenShareNotStarted,

    #[error("захват экрана: {0}")]
    Capture(String),

    #[error("видео: {0}")]
    Video(String),

    #[error("некорректный размер кадра: {width}x{height}")]
    InvalidFrameSize { width: u32, height: u32 },

    #[error("участник не найден: {0}")]
    ParticipantNotFound(String),

    #[error("некорректная конфигурация: {0}")]
    Config(String),

    #[error("операция отменена")]
    Cancelled,
}

pub type VoiceResult<T> = Result<T, VoiceError>;
