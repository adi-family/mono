use std::path::{Path, PathBuf};

use crate::plugin::PluginError;

pub fn queue_dir(workforce_dir: &Path, employee: &str, queue_name: &str) -> PathBuf {
    workforce_dir.join(employee).join("queue").join(queue_name)
}

pub fn push(dir: &Path, data: &[u8]) -> Result<(), PluginError> {
    std::fs::create_dir_all(dir).map_err(|e| PluginError::new(format!("queue dir: {e}")))?;

    clear_stale_lock(dir, "send.lock");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| PluginError::new(format!("tokio: {e}")))?;

    rt.block_on(async {
        let mut sender =
            yaque::Sender::open(dir).map_err(|e| PluginError::new(format!("queue open: {e}")))?;
        sender
            .send(data.to_vec())
            .await
            .map_err(|e| PluginError::new(format!("queue send: {e}")))
    })
}

pub fn push_string(dir: &Path, msg: &str) -> Result<(), PluginError> {
    push(dir, msg.as_bytes())
}

pub struct QueueReceiver {
    inner: yaque::Receiver,
    rt: tokio::runtime::Runtime,
}

impl QueueReceiver {
    pub fn open(dir: &Path) -> Result<Self, PluginError> {
        std::fs::create_dir_all(dir).map_err(|e| PluginError::new(format!("queue dir: {e}")))?;

        clear_stale_lock(dir, "recv.lock");

        let inner = yaque::ReceiverBuilder::new()
            .save_every_nth(Some(1))
            .open(dir)
            .map_err(|e| PluginError::new(format!("queue open: {e}")))?;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| PluginError::new(format!("tokio: {e}")))?;

        Ok(Self { inner, rt })
    }

    /// Read one message from the queue. Returns immediately if no message available.
    pub fn recv_once<F>(&mut self, on_msg: F)
    where
        F: FnOnce(&str),
    {
        let got = self.rt.block_on(async {
            match tokio::time::timeout(std::time::Duration::from_millis(500), self.inner.recv())
                .await
            {
                Ok(Ok(guard)) => Some(guard),
                _ => None,
            }
        });

        if let Some(guard) = got {
            let msg = String::from_utf8_lossy(&guard).to_string();
            on_msg(&msg);
            // A failed commit re-delivers the message next receive; log it.
            if let Err(e) = guard.commit() {
                eprintln!("[queue] commit failed (message will re-deliver): {e}");
            }
        }
    }

    /// Block and process messages indefinitely.
    pub fn recv_blocking<F>(&mut self, on_msg: F)
    where
        F: Fn(&str),
    {
        loop {
            let got = self.rt.block_on(async {
                match tokio::time::timeout(std::time::Duration::from_secs(2), self.inner.recv())
                    .await
                {
                    Ok(Ok(guard)) => Some(guard),
                    _ => None,
                }
            });

            if let Some(guard) = got {
                let msg = String::from_utf8_lossy(&guard).to_string();
                on_msg(&msg);
                // A failed commit re-delivers the message next receive; log it.
                if let Err(e) = guard.commit() {
                    eprintln!("[queue] commit failed (message will re-deliver): {e}");
                }
            }
        }
    }
}

fn clear_stale_lock(dir: &Path, lock_name: &str) {
    let lock_path = dir.join(lock_name);
    let content = match std::fs::read_to_string(&lock_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let pid: Option<u32> = content
        .lines()
        .find_map(|l| l.strip_prefix("pid="))
        .and_then(|s| s.trim().parse().ok());
    let Some(pid) = pid else {
        let _ = std::fs::remove_file(&lock_path);
        return;
    };
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !alive {
        let _ = std::fs::remove_file(&lock_path);
    }
}
