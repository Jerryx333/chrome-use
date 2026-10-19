//! The gateway's only way into the panel service. Panel calls are blocking
//! one-request connections, so they run on blocking threads and share a few
//! slots: the service accepts at most 16 at once and the desktop panel needs
//! some too.

use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

const SLOTS: usize = 4;
const POLL_BUDGET: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct Panel {
    slots: Arc<Semaphore>,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(SLOTS)),
        }
    }
}

impl Panel {
    /// A phone's request: may start the profile's service, as the desktop
    /// panel's requests do.
    pub async fn call(
        &self,
        operation: &str,
        params: Value,
        profile: &str,
    ) -> Result<Value, String> {
        let operation = operation.to_owned();
        let profile = profile.to_owned();
        self.run(move || {
            crate::agent_panel::handle_for_profile(&operation, &params, Some(&profile))
        })
        .await
    }

    /// Background polling: only a running service is asked; none is started.
    pub async fn poll(
        &self,
        operation: &str,
        params: Value,
        profile: &str,
    ) -> Result<Value, String> {
        let operation = operation.to_owned();
        let profile = profile.to_owned();
        self.run(move || {
            crate::agent_panel::call_running_service(&operation, &params, &profile, POLL_BUDGET)
        })
        .await
    }

    async fn run(
        &self,
        work: impl FnOnce() -> Result<Value, String> + Send + 'static,
    ) -> Result<Value, String> {
        let slot = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "The gateway is shutting down")?;
        // The slot lives with the blocking call: if the caller is dropped
        // (connection closed), the call still runs to its end holding it.
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            work()
        })
        .await
        .map_err(|_| "The panel call did not finish".to_string())?
    }
}
