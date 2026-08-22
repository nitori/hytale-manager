//! Where the server's output goes, and how a stop can be asked for without a signal.

use std::sync::Arc;

use tokio::sync::Notify;

/// Receives the server's console output, one line at a time.
///
/// `'static` because the forwarding tasks outlive the borrow of the reporter.
pub trait OutputSink: Send + Sync + 'static {
    fn line(&self, line: String);
}

#[derive(Clone)]
pub enum Output {
    /// The child writes straight to our stdout and stderr.
    Inherit,
    /// Lines are piped and handed to a sink — a UI drawing its own output pane.
    To(Arc<dyn OutputSink>),
}

impl Output {
    pub fn is_captured(&self) -> bool {
        matches!(self, Self::To(_))
    }
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inherit => f.write_str("Inherit"),
            Self::To(_) => f.write_str("To(..)"),
        }
    }
}

/// Asks the supervisor to stop, for callers that will not get a signal.
///
/// A UI in raw mode receives Ctrl-C as a key event rather than `SIGINT`, so it needs a way
/// to request the same shutdown the signal handler would.
#[derive(Clone, Debug, Default)]
pub struct StopHandle {
    notify: Arc<Notify>,
}

impl StopHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Request a graceful stop. Calling it again is the operator insisting, exactly as a
    /// second Ctrl-C would be.
    pub fn stop(&self) {
        self.notify.notify_one();
    }

    pub(crate) async fn requested(&self) {
        self.notify.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The UI can ask for a stop between the child spawning and the supervisor reaching
    /// its select, so the request has to survive having no listener.
    #[tokio::test]
    async fn a_stop_with_nobody_waiting_yet_is_kept() {
        let handle = StopHandle::new();
        handle.stop();
        tokio::time::timeout(std::time::Duration::from_millis(200), handle.requested())
            .await
            .expect("a stop requested before the wait should still arrive");
    }

    #[tokio::test]
    async fn a_waiting_supervisor_is_woken() {
        let handle = StopHandle::new();
        let waiter = handle.clone();
        let parked = tokio::spawn(async move { waiter.requested().await });
        tokio::task::yield_now().await;

        handle.stop();
        tokio::time::timeout(std::time::Duration::from_millis(200), parked)
            .await
            .expect("a parked waiter should be woken")
            .expect("the waiting task should not panic");
    }

    #[tokio::test]
    async fn nothing_is_reported_without_a_request() {
        let handle = StopHandle::new();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), handle.requested())
                .await
                .is_err()
        );
    }
}
