use tokio_util::sync::CancellationToken;

/// Wrapper around CancellationToken for coordinated shutdown.
#[derive(Clone)]
pub struct ShutdownSignal {
    token: CancellationToken,
}

impl ShutdownSignal {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    pub fn trigger(&self) {
        self.token.cancel();
    }

    pub fn is_triggered(&self) -> bool {
        self.token.is_cancelled()
    }

    pub async fn wait(&self) {
        self.token.cancelled().await;
    }

    pub fn token(&self) -> &CancellationToken {
        &self.token
    }
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        Self::new()
    }
}
