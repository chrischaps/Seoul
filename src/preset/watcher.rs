//! File watcher that sends ReloadEvents on .toml/.wgsl changes in the presets dir.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use anyhow::{Context, Result};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::info;

#[derive(Debug, Clone)]
pub enum ReloadEvent {
    FileChanged(PathBuf),
}

pub fn spawn(dir: &Path) -> Result<(RecommendedWatcher, Receiver<ReloadEvent>)> {
    let (tx, rx) = channel();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else {
            return;
        };
        if !matches!(
            event.kind,
            EventKind::Modify(_) | EventKind::Create(_)
        ) {
            return;
        }
        for path in event.paths {
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext == "toml" || ext == "wgsl" {
                // If the receiver is dropped the main thread exited; ignore.
                let _ = tx.send(ReloadEvent::FileChanged(path));
            }
        }
    })
    .context("create preset file watcher")?;

    watcher
        .watch(dir, RecursiveMode::Recursive)
        .context("watch preset dir")?;

    info!(dir = %dir.display(), "preset watcher started");
    Ok((watcher, rx))
}
