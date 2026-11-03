//! Page-open timings: the shell marks when a page is opened, pages mark when their content is
//! on screen, and the gap is logged as `[perf] <page> ready in N ms`.
use std::sync::Mutex;
use std::time::Instant;

static OPENED: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// The user opened `page` (called by the shell on navigation).
pub fn opened(page: impl Into<String>) {
    *OPENED.lock().unwrap_or_else(|e| e.into_inner()) = Some((page.into(), Instant::now()));
}

/// `what` (a page's content) is ready: logs the time since the last [`opened`], once per open.
pub fn ready(what: &str) {
    let Some((page, at)) = OPENED.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    eprintln!("[perf] {what} ready in {} ms (opened {page})", at.elapsed().as_millis());
}
