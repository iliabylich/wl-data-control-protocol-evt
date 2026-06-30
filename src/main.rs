use anyhow::{Context as _, Result, bail};
use rustix::{buffer::spare_capacity, event::epoll, pipe::PipeFlags};
use std::{
    collections::{HashMap, VecDeque},
    os::fd::{AsFd, AsRawFd},
};
use wayland_client::{Connection, EventQueue, protocol::wl_seat::WlSeat};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::ExtDataControlDeviceV1,
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_source_v1::ExtDataControlSourceV1,
};

mod reader;
use reader::{ReadResult, Reader};

mod writer;
use writer::{WriteResult, Writer};

mod offer_seq;
use offer_seq::{FinishedOfferSeq, OfferSeq};

mod wl_event;
use wl_event::WlEvent;

mod wl_state;
use wl_state::WaylandState;

use crate::wl_event::{WlOfferEvent, WlRegistryEvent, WlSourceEvent};

struct State {
    wl_seat: WlSeat,
    ext_data_control_manager: ExtDataControlManagerV1,
    ext_data_control_device: ExtDataControlDeviceV1,

    running: bool,
    conn: Connection,
    queue: EventQueue<WaylandState>,
    wl: WaylandState,
    readers: HashMap<i32, Reader>,
    writers: HashMap<i32, Writer>,
    mime_mask: String,

    offer_seq: OfferSeq,

    source_to_text: HashMap<ExtDataControlSourceV1, String>,

    readers_queue: VecDeque<Reader>,
    writers_queue: VecDeque<Writer>,
}

#[derive(Debug)]
enum ConnectError {
    WaylandConnectError(wayland_client::ConnectError),
    WaylandDispatchError(wayland_client::DispatchError),
    NoSeat,
    Unsupported,
}

impl core::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WaylandConnectError(err) => write!(f, "WaylandConnectError({err}"),
            Self::WaylandDispatchError(err) => write!(f, "WaylandDispatchError({err}"),
            Self::NoSeat => write!(f, "NoSeat"),
            Self::Unsupported => write!(f, "Unsupported"),
        }
    }
}

impl core::error::Error for ConnectError {}

impl State {
    fn connect() -> Result<Self, ConnectError> {
        let conn = Connection::connect_to_env().map_err(ConnectError::WaylandConnectError)?;
        let mut wl = WaylandState::new();
        let mut queue = conn.new_event_queue::<WaylandState>();

        let registry = conn.display().get_registry(&queue.handle(), ());
        queue
            .roundtrip(&mut wl)
            .map_err(ConnectError::WaylandDispatchError)?;

        let mut wl_seat: Option<WlSeat> = None;
        let mut ext_data_control_manager: Option<ExtDataControlManagerV1> = None;

        while let Some(event) = wl.registry_events.pop_front() {
            match event {
                WlRegistryEvent::WlSeat { name, version } => {
                    wl_seat = Some(registry.bind(name, version, &queue.handle(), ()));
                }
                WlRegistryEvent::ExtDataControlManager { name, version } => {
                    ext_data_control_manager =
                        Some(registry.bind(name, version, &queue.handle(), ()));
                }
                WlRegistryEvent::Other => {}
            }
        }

        let wl_seat = wl_seat.ok_or(ConnectError::NoSeat)?;
        let ext_data_control_manager = ext_data_control_manager.ok_or(ConnectError::Unsupported)?;

        let ext_data_control_device =
            ext_data_control_manager.get_data_device(&wl_seat, &queue.handle(), ());

        Ok(State {
            wl_seat,
            ext_data_control_manager,
            ext_data_control_device,

            running: true,
            wl,
            queue,
            conn,
            readers: HashMap::new(),
            writers: HashMap::new(),
            mime_mask: format!(
                "application/x-wayland-clipboard-poll-pid-{}",
                std::process::id()
            ),

            offer_seq: OfferSeq::Empty,
            source_to_text: HashMap::new(),

            readers_queue: VecDeque::new(),
            writers_queue: VecDeque::new(),
        })
    }

    fn handle(&mut self, event: WlEvent) {
        match event {
            WlEvent::Offer(event) => self.handle_offer_event(event),
            WlEvent::Source(event) => self.handle_source_event(event),
        }
    }

    fn handle_offer_event(&mut self, event: WlOfferEvent) {
        match event {
            WlOfferEvent::DataOffer(offer) => {
                self.offer_seq.start(offer);
            }

            WlOfferEvent::MimeTime(offer, mime_type) => {
                self.offer_seq.extend(&offer, mime_type);
            }

            WlOfferEvent::Selection(Some(offer)) => match self.offer_seq.finish(offer) {
                FinishedOfferSeq::Ok(offer, mimes) => {
                    let mime_type_to_ask_for = if mimes.contains(&self.mime_mask) {
                        None
                    } else if mimes.contains(MIME_TYPE_TEXT_UTF8) {
                        Some(MIME_TYPE_TEXT_UTF8)
                    } else if mimes.contains(MIME_TYPE_TEXT) {
                        Some(MIME_TYPE_TEXT)
                    } else {
                        None
                    };

                    if let Some(mime_type) = mime_type_to_ask_for {
                        match rustix::pipe::pipe_with(PipeFlags::NONBLOCK) {
                            Ok((reader, writer)) => {
                                offer.receive(String::from(mime_type), writer.as_fd());
                                drop(writer);
                                self.readers_queue.push_back(Reader::new(reader, offer));
                            }
                            Err(err) => {
                                log::error!("failed to create pipe: {err:?}");
                                offer.destroy();
                            }
                        }
                    } else {
                        offer.destroy();
                    }
                }
                FinishedOfferSeq::Mismatch(prev, next) => {
                    log::error!("Something is wrong with the sequence of events");
                    log::error!("Selection->offer doesn't match state->incoming");
                    prev.destroy();
                    next.destroy();
                }
                FinishedOfferSeq::Err(offer) => {
                    offer.destroy();
                }
            },
            WlOfferEvent::Selection(None) => {
                self.offer_seq.destroy();
            }

            WlOfferEvent::PrimarySelection(Some(offer)) => match self.offer_seq.finish(offer) {
                FinishedOfferSeq::Ok(same, _) => {
                    same.destroy();
                }
                FinishedOfferSeq::Mismatch(prev, next) => {
                    log::error!("Something is wrong with the sequence of events");
                    log::error!("PrimarySelection->offer doesn't match state->incoming");
                    prev.destroy();
                    next.destroy();
                }
                FinishedOfferSeq::Err(offer) => {
                    offer.destroy();
                }
            },
            WlOfferEvent::PrimarySelection(None) => {
                self.offer_seq.destroy();
            }

            WlOfferEvent::Finished => {
                log::warn!("ExtDataControlDeviceV1 has finished");
                self.running = false;
            }
        }
    }

    fn handle_source_event(&mut self, event: WlSourceEvent) {
        match event {
            WlSourceEvent::Requested(source, mime_type, fd) => {
                if mime_type == MIME_TYPE_TEXT_UTF8 || mime_type == MIME_TYPE_TEXT {
                    if let Some(text) = self.source_to_text.get(&source).cloned() {
                        match Writer::new(fd, text) {
                            Ok(writer) => self.writers_queue.push_back(writer),
                            Err(err) => log::error!("{err:?}"),
                        }
                    }
                }
            }
            WlSourceEvent::Cancelled(source) => {
                self.source_to_text.remove(&source);
                source.destroy();
            }
        }
    }

    fn cleanup(&mut self) -> Result<()> {
        self.ext_data_control_device.destroy();
        self.wl_seat.release();
        self.ext_data_control_manager.destroy();

        for reader in core::mem::take(&mut self.readers).into_values() {
            reader.destroy();
        }
        self.writers.clear();
        self.offer_seq.destroy();
        for source in core::mem::take(&mut self.source_to_text).into_keys() {
            source.destroy()
        }
        for reader in core::mem::take(&mut self.readers_queue) {
            reader.destroy();
        }

        self.writers_queue.clear();

        self.queue.flush()?;
        Ok(())
    }
}

const MIME_TYPE_TEXT: &str = "text/plain";
const MIME_TYPE_TEXT_UTF8: &str = "text/plain;charset=utf-8";

fn main() -> Result<()> {
    env_logger::init();

    let mut state = State::connect()?;

    let source = state
        .ext_data_control_manager
        .create_data_source(&state.queue.handle(), ());
    source.offer("text/plain;charset=utf-8".to_string());
    source.offer("text/plain".to_string());
    source.offer(state.mime_mask.clone());

    state.ext_data_control_device.set_selection(Some(&source));
    state.source_to_text.insert(source, String::from("FOO"));
    state.queue.flush()?;

    let epoll = epoll::create(epoll::CreateFlags::CLOEXEC)?;
    epoll::add(
        &epoll,
        state.conn.as_fd(),
        epoll::EventData::new_u64(state.conn.as_fd().as_raw_fd() as u64),
        epoll::EventFlags::IN,
    )?;
    let mut epoll_events = Vec::with_capacity(16);

    while state.running {
        state.queue.flush()?;
        state.queue.dispatch_pending(&mut state.wl)?;

        while let Some(reader) = state.readers_queue.pop_front() {
            log::trace!("Got new reader, adding to state {:?}", reader.as_raw_fd());
            epoll::add(
                &epoll,
                &reader,
                epoll::EventData::new_u64(reader.as_raw_fd() as u64),
                epoll::EventFlags::IN,
            )?;
            state.readers.insert(reader.as_raw_fd(), reader);
        }

        while let Some(writer) = state.writers_queue.pop_front() {
            log::trace!("Got new writer, adding to state {:?}", writer.as_raw_fd());
            epoll::add(
                &epoll,
                &writer,
                epoll::EventData::new_u64(writer.as_raw_fd() as u64),
                epoll::EventFlags::OUT,
            )?;
            state.writers.insert(writer.as_raw_fd(), writer);
        }

        state.queue.flush()?;
        let wl_read_guard = state
            .queue
            .prepare_read()
            .context("failed to create ReadEventsGuard")?;

        log::trace!("epoll_wait()...");
        epoll::wait(&epoll, spare_capacity(&mut epoll_events), None)?;
        let EpollResult {
            wl_is_readable,
            readers,
            writers,
        } = EpollResult::new(&epoll_events, &state)?;
        epoll_events.clear();

        if wl_is_readable {
            wl_read_guard.read()?;
            state.queue.dispatch_pending(&mut state.wl)?;

            while let Some(event) = state.wl.events.pop_front() {
                state.handle(event);
            }
        } else {
            drop(wl_read_guard);
        }

        use std::collections::hash_map::Entry;

        for fd in readers.dead {
            if let Entry::Occupied(entry) = state.readers.entry(fd) {
                let reader = entry.remove();
                epoll::delete(&epoll, &reader)?;
                reader.destroy();
            }
        }

        for fd in writers.dead {
            if let Entry::Occupied(entry) = state.writers.entry(fd) {
                let writer = entry.remove();
                epoll::delete(&epoll, &writer)?;
            }
        }

        for fd in readers.ready {
            if let Entry::Occupied(mut entry) = state.readers.entry(fd) {
                let reader = entry.get_mut();
                log::trace!("Reading {:?}", reader.as_raw_fd());
                match reader.read() {
                    Ok(ReadResult::Done(text)) => {
                        log::trace!("Got text {text:?}");
                        epoll::delete(&epoll, &mut *reader)?;
                        reader.destroy();
                        entry.remove();

                        if text == "EXIT" {
                            state.running = false;
                        }
                    }
                    Ok(ReadResult::Pending) => {}
                    Err(err) => {
                        log::error!("reader {:?} returned error {err:?}", reader.as_raw_fd());
                        epoll::delete(&epoll, &mut *reader)?;
                        reader.destroy();
                        entry.remove();
                    }
                }
            }
        }

        for fd in writers.ready {
            if let Entry::Occupied(mut entry) = state.writers.entry(fd) {
                let writer = entry.get_mut();
                log::trace!("Writing {:?}", writer.as_raw_fd());
                match writer.write() {
                    Ok(WriteResult::Done) => {
                        log::trace!("Done writing to {:?}", writer.as_raw_fd());
                        epoll::delete(&epoll, writer)?;
                        entry.remove();
                    }
                    Ok(WriteResult::Pending) => {}
                    Err(err) => {
                        log::error!("writer {:?} returned error {err:?}", writer.as_raw_fd());
                        epoll::delete(&epoll, writer)?;
                        entry.remove();
                    }
                }
            }
        }
    }

    state.cleanup()?;

    Ok(())
}

#[derive(Default)]
struct FdSet {
    ready: Vec<i32>,
    dead: Vec<i32>,
}

#[derive(Default)]
struct EpollResult {
    wl_is_readable: bool,
    readers: FdSet,
    writers: FdSet,
}

impl EpollResult {
    fn new(events: &[epoll::Event], state: &State) -> Result<Self> {
        let mut wl_is_readable = false;
        let mut readers = FdSet::default();
        let mut writers = FdSet::default();

        for event in events {
            let fd = event.data.u64() as i32;
            let revents: epoll::EventFlags = event.flags;

            if fd == state.conn.as_fd().as_raw_fd() {
                if revents.intersects(epoll::EventFlags::HUP | epoll::EventFlags::ERR) {
                    bail!("Wayland returned revents {revents:?}");
                } else if revents.contains(epoll::EventFlags::IN) {
                    wl_is_readable = true;
                }
            } else if state.readers.contains_key(&fd) {
                if revents.intersects(epoll::EventFlags::ERR) {
                    log::error!("Reader with FD {fd} returned revents {revents:?}, removing it");
                    readers.dead.push(fd);
                } else if revents.intersects(epoll::EventFlags::IN | epoll::EventFlags::HUP) {
                    readers.ready.push(fd);
                }
            } else if state.writers.contains_key(&fd) {
                if revents.intersects(epoll::EventFlags::ERR | epoll::EventFlags::HUP) {
                    log::error!("Writer with FD {fd} returned revents {revents:?}, removing it");
                    writers.dead.push(fd);
                } else if revents.contains(epoll::EventFlags::OUT) {
                    writers.ready.push(fd);
                }
            }
        }

        Ok(EpollResult {
            wl_is_readable,
            readers,
            writers,
        })
    }
}
