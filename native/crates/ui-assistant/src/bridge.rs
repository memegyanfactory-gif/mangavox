//! Runs closures on the GPUI thread for assistant tools (they run on tokio), so a tool changes the
//! same entities and globals the pages read. The app drains the queue in a task spawned by
//! [`install`]; each job gets the main window.
use std::sync::OnceLock;

use futures::channel::{mpsc, oneshot};
use futures::StreamExt as _;
use gpui_kit::*;

type Job = Box<dyn FnOnce(&mut Window, &mut App) + Send>;

static QUEUE: OnceLock<mpsc::UnboundedSender<Job>> = OnceLock::new();

pub fn install(window: AnyWindowHandle, cx: &mut App) {
    let (tx, mut rx) = mpsc::unbounded::<Job>();
    if QUEUE.set(tx).is_err() {
        return;
    }
    cx.spawn(async move |cx| {
        while let Some(job) = rx.next().await {
            let _ = window.update(cx, |_, window, cx| job(window, cx));
        }
    })
    .detach();
}

/// Runs `f` on the UI thread with the main window and returns its result.
pub async fn on_ui<R: Send + 'static>(f: impl FnOnce(&mut Window, &mut App) -> R + Send + 'static) -> Result<R, String> {
    let queue = QUEUE.get().ok_or("The app is still starting")?;
    let (tx, rx) = oneshot::channel();
    queue
        .unbounded_send(Box::new(move |w, cx| {
            let _ = tx.send(f(w, cx));
        }))
        .map_err(|_| "The app is closing".to_string())?;
    rx.await.map_err(|_| "The app window is gone".to_string())
}
