use hash_hasher::HashedMap;
use mediasoup_sys::fbs::notification;
use nohash_hasher::IntMap;
use parking_lot::Mutex;
use serde::Deserialize;
use std::sync::{Arc, Weak};
use uuid::Uuid;

struct EventHandlersList<F> {
    index: usize,
    callbacks: IntMap<usize, F>,
}

impl<F> Default for EventHandlersList<F> {
    fn default() -> Self {
        Self {
            index: 0,
            callbacks: IntMap::default(),
        }
    }
}

#[derive(Clone)]
pub(super) struct EventHandlers<F> {
    handlers: Arc<Mutex<HashedMap<SubscriptionTarget, EventHandlersList<F>>>>,
}

impl<F: Sized + Send + Sync + 'static> EventHandlers<F> {
    pub(super) fn new() -> Self {
        let handlers = Arc::<Mutex<HashedMap<SubscriptionTarget, EventHandlersList<F>>>>::default();
        Self { handlers }
    }

    pub(super) fn add(&self, target_id: SubscriptionTarget, callback: F) -> SubscriptionHandler {
        let index;
        {
            let mut event_handlers = self.handlers.lock();
            let list = event_handlers.entry(target_id.clone()).or_default();
            index = list.index;
            list.index += 1;
            list.callbacks.insert(index, callback);
            drop(event_handlers);
        }

        SubscriptionHandler::new({
            let event_handlers_weak = Arc::downgrade(&self.handlers);

            Box::new(move || {
                if let Some(event_handlers) = event_handlers_weak.upgrade() {
                    // We store removed handler here in order to drop after `event_handlers` lock is
                    // released, otherwise handler will be dropped on removal from callbacks
                    // immediately and if it happens to hold another entity that held subscription
                    // handler, we may arrive here again trying to acquire lock that we didn't
                    // release yet. By dropping callback only when lock is released we avoid
                    // deadlocking.
                    let removed_handler;
                    {
                        let mut handlers = event_handlers.lock();
                        let is_empty = {
                            let list = handlers.get_mut(&target_id).unwrap();
                            removed_handler = list.callbacks.remove(&index);
                            list.callbacks.is_empty()
                        };
                        if is_empty {
                            handlers.remove(&target_id);
                        }
                    }
                    drop(removed_handler);
                }
            })
        })
    }

    pub(super) fn downgrade(&self) -> WeakEventHandlers<F> {
        WeakEventHandlers {
            handlers: Arc::downgrade(&self.handlers),
        }
    }
}

impl EventHandlers<Arc<dyn Fn(notification::NotificationRef<'_>) + Send + Sync + 'static>> {
    pub(super) fn call_callbacks_with_single_value(
        &self,
        target_id: &SubscriptionTarget,
        value: notification::NotificationRef<'_>,
    ) {
        // Callbacks must not run under `handlers` lock: a callback, or a value it creates and
        // drops (like strong `Producer` handles in volumes of audio level observer), may release
        // the last reference to an entity, whose `SubscriptionHandler` needs this same
        // non-reentrant lock on this same thread when dropped.
        //
        // Every subscriber in this crate registers one callback for its own target, in which case
        // this clones one `Arc` and doesn't allocate.
        let (first_callback, other_callbacks) = {
            let handlers = self.handlers.lock();
            let Some(list) = handlers.get(target_id) else {
                return;
            };
            let mut callbacks = list.callbacks.values().cloned();

            (callbacks.next(), callbacks.collect::<Vec<_>>())
        };

        for callback in first_callback.iter().chain(&other_callbacks) {
            callback(value);
        }
    }
}

#[derive(Clone)]
pub(super) struct WeakEventHandlers<F> {
    handlers: Weak<Mutex<HashedMap<SubscriptionTarget, EventHandlersList<F>>>>,
}

impl<F> WeakEventHandlers<F> {
    pub(super) fn upgrade(&self) -> Option<EventHandlers<F>> {
        self.handlers
            .upgrade()
            .map(|handlers| EventHandlers { handlers })
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize)]
#[serde(untagged)]
pub(crate) enum SubscriptionTarget {
    Uuid(Uuid),
    String(String),
}

/// Subscription handler, will remove corresponding subscription when dropped
pub(crate) struct SubscriptionHandler {
    remove_callback: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl SubscriptionHandler {
    fn new(remove_callback: Box<dyn FnOnce() + Send + Sync>) -> Self {
        Self {
            remove_callback: Some(remove_callback),
        }
    }
}

impl Drop for SubscriptionHandler {
    fn drop(&mut self) {
        let remove_callback = self.remove_callback.take().unwrap();
        remove_callback();
    }
}
