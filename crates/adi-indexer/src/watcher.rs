// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::{Config, Result};
use fs2::FileExt;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[derive(Debug)]
pub struct Watcher {
    project_path: PathBuf,
    config: Arc<Config>,
    tx: mpsc::UnboundedSender<Vec<PathBuf>>,
}

impl Watcher {
    #[must_use]
    pub fn new(
        project_path: PathBuf,
        config: Arc<Config>,
        on_change: mpsc::UnboundedSender<Vec<PathBuf>>,
    ) -> Self {
        Self {
            project_path,
            config,
            tx: on_change,
        }
    }

    pub fn start(self) -> Result<()> {
        let project_path = self.project_path.clone();
        let config = self.config.clone();
        let tx = self.tx.clone();

        // Try to acquire lock
        let lock_path = project_path.join(".adi/.watch.lock");

        std::thread::spawn(move || {
            // Create lock file
            let lock_file = match File::create(&lock_path) {
                Ok(file) => file,
                Err(e) => {
                    error!("Failed to create lock file: {}", e);
                    return;
                }
            };

            // Try to acquire exclusive lock (non-blocking)
            match lock_file.try_lock_exclusive() {
                Ok(()) => {
                    info!("Acquired file watcher lock");

                    // Run the watcher loop
                    if let Err(e) = Self::watch_loop(project_path, config, tx) {
                        error!("File watcher error: {}", e);
                    }

                    // Lock is automatically released when lock_file is dropped
                    let _ = lock_file.unlock();
                    info!("Released file watcher lock");
                }
                Err(e) => {
                    warn!(
                        "Another process is already watching this project. Skipping file watcher initialization. ({})",
                        e
                    );
                }
            }
        });

        Ok(())
    }

    fn watch_loop(
        project_path: PathBuf,
        config: Arc<Config>,
        tx: mpsc::UnboundedSender<Vec<PathBuf>>,
    ) -> Result<()> {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                if let Ok(event) = res {
                    let _ = event_tx.send(event);
                }
            },
            notify::Config::default(),
        )?;

        watcher.watch(&project_path, RecursiveMode::Recursive)?;
        info!("File watcher started for: {}", project_path.display());

        // Debounce mechanism
        let mut pending_paths: Vec<PathBuf> = Vec::new();
        let debounce_duration = Duration::from_secs(1);
        let mut last_event_time = std::time::Instant::now();

        loop {
            // Check for new events
            while let Ok(event) = event_rx.try_recv() {
                if should_process_event(&event) {
                    for path in event.paths {
                        if should_index_path(&path, &project_path, &config)
                            && let Ok(relative) = path.strip_prefix(&project_path)
                        {
                            debug!("File changed: {}", relative.display());
                            pending_paths.push(relative.to_path_buf());
                        }
                    }
                    last_event_time = std::time::Instant::now();
                }
            }

            // Send pending changes if debounce period has elapsed
            if !pending_paths.is_empty() && last_event_time.elapsed() >= debounce_duration {
                let paths = std::mem::take(&mut pending_paths);
                info!("Triggering reindex for {} changed files", paths.len());

                if tx.send(paths).is_err() {
                    warn!("Change handler has been dropped, stopping watcher");
                    break;
                }
            }

            // Small sleep to avoid busy waiting
            std::thread::sleep(Duration::from_millis(100));
        }

        Ok(())
    }
}

fn should_process_event(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn should_index_path(path: &Path, project_path: &Path, config: &Config) -> bool {
    // Ignore .adi directory (including lock file)
    if path.starts_with(project_path.join(".adi")) {
        return false;
    }

    // Ignore .git directory
    if path.starts_with(project_path.join(".git")) {
        return false;
    }

    // Ignore lock file specifically (extra safety)
    if path.ends_with(".watch.lock") {
        return false;
    }

    // Only process files, not directories
    if !path.is_file() {
        return false;
    }

    // Check against ignore patterns
    if let Ok(relative_path) = path.strip_prefix(project_path) {
        let ignore_builder = ignore::gitignore::GitignoreBuilder::new(project_path);

        // Add patterns from config
        let mut builder = ignore_builder;
        for pattern in &config.ignore.patterns {
            let _ = builder.add_line(None, pattern);
        }

        if let Ok(ignore) = builder.build() {
            let matched = ignore.matched(relative_path, path.is_dir());
            if matched.is_ignore() {
                return false;
            }
        }
    }

    true
}
