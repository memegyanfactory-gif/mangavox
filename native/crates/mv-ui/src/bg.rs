//! Background work. GPUI has its own executor; network crates need tokio.
//! `bg::run(fut)` runs a future on a shared tokio runtime and returns a future
//! that GPUI can await (e.g. inside `cx.spawn`).
use std::future::Future;
use std::sync::OnceLock;

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .thread_name("mv-bg")
            // Below normal priority under the system resource limit, so the UI thread wins.
            .on_thread_start(mv_core::limits::lower_current_thread)
            .build()
            .expect("tokio runtime")
    })
}

/// Run `fut` on the tokio runtime; await the returned future from GPUI code.
pub fn run<F, T>(fut: F) -> impl Future<Output = anyhow::Result<T>>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = futures::channel::oneshot::channel();
    runtime().spawn(async move {
        let _ = tx.send(fut.await);
    });
    async move { rx.await.map_err(|_| anyhow::anyhow!("background task was cancelled")) }
}

/// Run blocking work (SQLite, file IO) off the UI thread.
pub fn blocking<F, T>(f: F) -> impl Future<Output = anyhow::Result<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    run(async move { tokio::task::spawn_blocking(f).await.expect("blocking task panicked") })
}
