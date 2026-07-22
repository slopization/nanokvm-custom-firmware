use moonlink_core::ServerError;

#[derive(Debug, thiserror::Error)]
pub enum TokioServerError {
    #[error(transparent)]
    Server(#[from] ServerError),
    #[error("missing media provider")]
    MissingMediaProvider,
    #[error("missing input sink")]
    MissingInputSink,
    #[error("missing pairing store")]
    MissingPairingStore,
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("server task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}
