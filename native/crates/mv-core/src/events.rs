//! Small typed broadcast bus for background work -> UI. Each subscriber gets its own queue;
//! dropped receivers are pruned on the next publish.
use serde_json::Value;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

pub struct EventBus<T: Clone> {
    subs: Mutex<Vec<Sender<T>>>,
}

impl<T: Clone> Default for EventBus<T> {
    fn default() -> Self {
        Self { subs: Mutex::new(Vec::new()) }
    }
}

impl<T: Clone> EventBus<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> Receiver<T> {
        let (tx, rx) = channel();
        self.subs.lock().unwrap().push(tx);
        rx
    }

    /// Sends a clone to every live subscriber; returns how many received it.
    pub fn publish(&self, event: T) -> usize {
        let mut subs = self.subs.lock().unwrap();
        subs.retain(|s| s.send(event.clone()).is_ok());
        subs.len()
    }

    pub fn subscriber_count(&self) -> usize {
        self.subs.lock().unwrap().len()
    }
}

/// Events the native app shares between background tasks and views.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    ConfigChanged { key: String, value: Value },
    LibraryChanged,
    DownloadProgress { chapter_id: String, done: u32, total: u32 },
    DownloadStatus { chapter_id: String, status: String },
    Custom { topic: String, payload: Value },
}

pub type AppEventBus = EventBus<AppEvent>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_and_prune() {
        let bus = AppEventBus::new();
        let a = bus.subscribe();
        let b = bus.subscribe();
        assert_eq!(bus.publish(AppEvent::LibraryChanged), 2);
        assert_eq!(a.try_recv().unwrap(), AppEvent::LibraryChanged);
        assert_eq!(b.try_recv().unwrap(), AppEvent::LibraryChanged);
        drop(b);
        assert_eq!(bus.publish(AppEvent::DownloadStatus { chapter_id: "c".into(), status: "queued".into() }), 1);
        assert_eq!(bus.subscriber_count(), 1);
        assert!(matches!(a.try_recv().unwrap(), AppEvent::DownloadStatus { .. }));
    }

    #[test]
    fn works_across_threads() {
        let bus = std::sync::Arc::new(EventBus::<u32>::new());
        let rx = bus.subscribe();
        let b2 = bus.clone();
        std::thread::spawn(move || {
            for i in 0..3 {
                b2.publish(i);
            }
        })
        .join()
        .unwrap();
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![0, 1, 2]);
    }
}
