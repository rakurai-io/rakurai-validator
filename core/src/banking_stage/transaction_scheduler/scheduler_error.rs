use thiserror::Error;

#[derive(Debug, Error)]
#[repr(C)]
pub enum SchedulerError {
    #[error("Sending channel disconnected: {0}")]
    DisconnectedSendChannel(&'static str),
    #[error("Recv channel disconnected: {0}")]
    DisconnectedRecvChannel(&'static str),
    #[error("Scheduler panicked: {0}")]
    Panic(String),
}
