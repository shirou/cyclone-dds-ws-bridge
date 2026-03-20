#[derive(Debug, thiserror::Error)]
pub enum DdsError {
    #[error("failed to create DomainParticipant: DDS error code {0}")]
    CreateParticipant(i32),
    #[error("failed to create topic: DDS error code {0}")]
    CreateTopic(i32),
    #[error("failed to create writer: DDS error code {0}")]
    CreateWriter(i32),
    #[error("failed to create reader: DDS error code {0}")]
    CreateReader(i32),
    #[error("DDS write failed: error code {0}")]
    Write(i32),
    #[error("DDS dispose failed: error code {0}")]
    Dispose(i32),
    #[error("DDS writedispose failed: error code {0}")]
    WriteDispose(i32),
    #[error("DDS read/take failed: error code {0}")]
    Read(i32),
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("writer not found: {0}")]
    WriterNotFound(u32),
    #[error("writer ownership violation: writer {0} belongs to another session")]
    WriterOwnership(u32),
}
