use mio::*;
use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::time::Duration;
use slab::Slab;

pub trait EventHandler {
    fn on_ready(&mut self, selector: &mut Selector, event: Event);
}

impl<F> EventHandler for F where F: FnMut(&mut Selector, Event) {
    fn on_ready(&mut self, selector: &mut Selector, event: Event) {
        self(selector, event);
    }
}

// for convenience
impl<T: EventHandler> EventHandler for Rc<RefCell<T>> {
    fn on_ready(&mut self, selector: &mut Selector, event: Event) {
        self.borrow_mut().on_ready(selector, event);
    }
}

pub struct Selector {
    poll: Poll,
    handlers: Slab<SelectionHandler, Token>,
    // tokens to be removed after all the current poll events are executed
    tokens_to_remove: Vec<Token>,
}

struct SelectionHandler {
    handler: Rc<RefCell<Box<EventHandler>>>,
    // registered on Poll, true when !interest.is_empty()
    registered: bool,
}

impl SelectionHandler {
    fn new(handler: Box<EventHandler>) -> Self {
        Self {
            handler: Rc::new(RefCell::new(handler)),
            registered: true,
        }
    }
}

impl Selector {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            poll: Poll::new()?,
            handlers: Slab::with_capacity(1024),
            tokens_to_remove: Vec::new(),
        })
    }

    pub fn register<E>(&mut self, handle: &E, handler: Box<EventHandler>,
                   interest: Ready, opts: PollOpt) -> io::Result<Token>
            where E: Evented + ?Sized {
        let token = self.handlers.insert(SelectionHandler::new(handler))
                        .map_err(|_| io::Error::new(io::ErrorKind::Other, "Cannot allocate slab slot"))?;
        self.poll.register(handle, token, interest, opts)?;
        Ok(token)
    }

    pub fn reregister<E>(&mut self, handle: &E, token: Token,
                   interest: Ready, opts: PollOpt) -> io::Result<()>
            where E: Evented + ?Sized {
        // a Poll does not accept to register an empty Ready
        // for simplifying its usage, expose an API that does
        let selection_handler = self.handlers.get_mut(token).expect("Token not found");
        if interest.is_empty() {
            if selection_handler.registered {
                selection_handler.registered = false;
                self.poll.deregister(handle)?;
            }
            Ok(())
        } else {
            if !selection_handler.registered {
                selection_handler.registered = true;
                self.poll.register(handle, token, interest, opts)
            } else {
                self.poll.reregister(handle, token, interest, opts)
            }
        }
    }

    pub fn deregister<E>(&mut self, handle: &E, token: Token) -> io::Result<()>
            where E: Evented + ?Sized {
        let selection_handler = self.handlers.get_mut(token).expect("Token not found");
        if selection_handler.registered {
            self.poll.deregister(handle)?;
        }
        // remove them before next poll()
        self.tokens_to_remove.push(token);
        Ok(())
    }

    pub fn clean_removed_tokens(&mut self) {
        for &token in &self.tokens_to_remove {
            self.handlers.remove(token).expect("Token not found");
        }
        self.tokens_to_remove.clear();
    }

    pub fn poll(&mut self, events: &mut Events, timeout: Option<Duration>) -> io::Result<usize> {
        self.poll.poll(events, timeout)
    }

    pub fn run_handler(&mut self, event: Event) {
        let handler = self.handlers.get_mut(event.token()).expect("Token not found").handler.clone();
        handler.borrow_mut().on_ready(self, event);
    }
}
