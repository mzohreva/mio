use crossbeam_channel as mpmc;
use event_imp::Event;
use std::collections::HashMap;
use std::fmt;
use std::iter;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use {io, Ready, Token};

pub struct Selector {
    id: usize,
    event_rx: mpmc::Receiver<(RegistrationId, EventKind)>,
    shared_inner: Arc<SelectorSharedInner>,
}

struct SelectorSharedInner {
    event_tx: mpmc::Sender<(RegistrationId, EventKind)>,
    registrations: Mutex<HashMap<RegistrationId, (Token, Ready)>>,
}

impl Selector {
    pub fn new() -> io::Result<Selector> {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        let (event_tx, event_rx) = mpmc::unbounded();
        Ok(Selector {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            event_rx,
            shared_inner: Arc::new(SelectorSharedInner {
                event_tx,
                registrations: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn select(
        &self,
        events: &mut Events,
        awakener: Token,
        timeout: Option<Duration>,
    ) -> io::Result<bool> {
        events.clear();
        let first = match timeout {
            Some(timeout) => match self.event_rx.recv_timeout(timeout) {
                Err(mpmc::RecvTimeoutError::Timeout) => return Ok(false),
                Err(mpmc::RecvTimeoutError::Disconnected) => panic!("event channel closed unexpectedly"),
                Ok(v) => v,
            },
            None => self.event_rx.recv().expect("event channel closed unexpectedly"),
        };
        let mut ret = false;
        let registrations = self.shared_inner.registrations.lock().unwrap();
        for (reg_id, kind) in iter::once(first).chain(self.event_rx.try_iter()) {
            if let Some((token, interest)) = registrations.get(&reg_id) {
                if *token == awakener {
                    ret = true;
                } else if kind.matches_interest(interest) {
                    events.push_event(Event::new(kind.to_readiness(), *token));
                }
            }
            if events.len() == events.capacity() {
                break;
            }
        }
        Ok(ret)
    }
}

impl fmt::Debug for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Selector").field("id", &self.id).finish()
    }
}

pub(crate) struct Registration {
    id: RegistrationId,
    shared_inner: Arc<SelectorSharedInner>,
    token: Token,
    interest: Ready,
}

impl Registration {
    pub fn new(selector: &Selector, token: Token, interest: Ready) -> Self {
        let id = RegistrationId::new();
        selector.shared_inner.registrations.lock().unwrap().insert(id, (token, interest));
        Registration {
            id,
            shared_inner: selector.shared_inner.clone(),
            token,
            interest: interest,
        }
    }

    pub fn change_details(&mut self, token: Token, interest: Ready) -> bool {
        if self.token == token && self.interest == interest {
            return false;
        }
        self.token = token;
        self.interest = interest;
        self.shared_inner.registrations.lock().unwrap().insert(self.id, (self.token, self.interest));
        true
    }

    pub fn push_event(&self, kind: EventKind) {
        if kind.matches_interest(&self.interest) {
            let _ = self.shared_inner.event_tx.send((self.id, kind));
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.shared_inner.registrations.lock().unwrap().remove(&self.id);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RegistrationId(usize);

impl RegistrationId {
    fn new() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug)]
pub(crate) enum EventKind {
    Readable,
    ReadClosed,
    ReadError,
    Writable,
    WriteClosed,
    WriteError,
}

impl EventKind {
    fn matches_interest(&self, interest: &Ready) -> bool {
        use self::EventKind::*;
        match self {
            Readable | ReadClosed => interest.is_readable(),
            Writable | WriteClosed => interest.is_writable(),
            // Always send error events
            ReadError | WriteError => true,
        }
    }

    fn to_readiness(&self) -> Ready {
        use self::EventKind::*;
        let mut ready = Ready::empty();
        match self {
            Readable | ReadClosed | ReadError => ready |= Ready::readable(),
            Writable | WriteClosed | WriteError => ready |= Ready::writable(),
        }
        ready
    }
}

#[derive(Debug)]
pub struct Events(Vec<Event>);

impl Events {
    pub fn with_capacity(cap: usize) -> Events {
        Events(Vec::with_capacity(cap))
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    pub fn get(&self, idx: usize) -> Option<Event> {
        self.0.get(idx).cloned()
    }

    pub fn push_event(&mut self, event: Event) {
        self.0.push(event);
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}
