//! Provider clients owned by the channels service. No other process starts these sockets.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adi_channels::RouterClient;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::info;

struct Task {
    shutdown: watch::Sender<bool>,
    connected: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

#[derive(Default)]
pub struct Live {
    tasks: Mutex<HashMap<String, Task>>,
}

impl Live {
    pub async fn ensure(&self, client: RouterClient) {
        let mut tasks = self.tasks.lock().await;
        if tasks.contains_key(&client.provider) {
            return;
        }
        let provider = client.provider.clone();
        let connected = Arc::clone(&client.connected);
        let status = Arc::clone(&connected);
        let (shutdown, mut cancel) = watch::channel(false);
        let rx = cancel.clone();
        let handle = tokio::spawn(async move {
            // Cancellation also covers a connect/TLS handshake stuck before the client's
            // own read loop starts observing its shutdown receiver.
            tokio::select! {
                () = client.run(rx) => {},
                _ = cancel.changed() => {},
            }
            status.store(false, Ordering::Relaxed);
        });
        info!(provider, "channel router client started");
        tasks.insert(
            provider,
            Task {
                shutdown,
                connected,
                handle,
            },
        );
    }

    /// Hold the task map until the old socket has actually closed, so a concurrent ensure
    /// cannot briefly start a second client for the same provider.
    pub async fn retain(&self, providers: &[String]) {
        let mut tasks = self.tasks.lock().await;
        let obsolete: Vec<_> = tasks
            .keys()
            .filter(|p| !providers.contains(p))
            .cloned()
            .collect();
        for provider in obsolete {
            if let Some(task) = tasks.remove(&provider) {
                let _ = task.shutdown.send(true);
                let _ = task.handle.await;
                info!(provider, "channel router client stopped");
            }
        }
    }

    pub async fn status(&self) -> BTreeMap<String, bool> {
        self.tasks
            .lock()
            .await
            .iter()
            .map(|(provider, task)| (provider.clone(), task.connected.load(Ordering::Relaxed)))
            .collect()
    }

    pub async fn stop_all(&self) {
        self.retain(&[]).await;
    }
}
