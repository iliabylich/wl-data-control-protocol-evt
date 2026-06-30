use anyhow::{Context as _, Result, bail};
use rustix::{buffer::spare_capacity, event::epoll, io::Errno, pipe::PipeFlags};
use std::{
    collections::HashMap,
    os::fd::{AsFd, AsRawFd, OwnedFd},
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
use offer_seq::OfferSeq;

mod wl_event;
use wl_event::WlEvent;

mod wl_state;
use wl_state::WlState;

mod mime_types;
use mime_types::MimeTypes;

use crate::wl_event::{WlOfferEvent, WlRegistryEvent, WlSourceEvent};

struct State {
    wl_seat: WlSeat,
    ext_data_control_manager: ExtDataControlManagerV1,
    ext_data_control_device: ExtDataControlDeviceV1,

    epoll: OwnedFd,
    running: bool,

    conn: Connection,
    queue: EventQueue<WlState>,
    wl: WlState,
    readers: HashMap<i32, Reader>,
    writers: HashMap<i32, Writer>,
    mime_types: MimeTypes,

    offer_seq: OfferSeq,

    source_to_text: HashMap<ExtDataControlSourceV1, String>,
}

#[derive(Debug)]
enum ConnectError {
    WaylandConnectError(wayland_client::ConnectError),
    WaylandDispatchError(wayland_client::DispatchError),
    NoSeat,
    Unsupported,
    FailedToCreateEpoll(Errno),
}

impl core::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WaylandConnectError(err) => write!(f, "WaylandConnectError({err})"),
            Self::WaylandDispatchError(err) => write!(f, "WaylandDispatchError({err})"),
            Self::NoSeat => write!(f, "NoSeat"),
            Self::Unsupported => write!(f, "Unsupported"),
            Self::FailedToCreateEpoll(err) => write!(f, "FailedToCreateEpoll({err})"),
        }
    }
}

impl core::error::Error for ConnectError {}

impl State {
    fn connect() -> Result<Self, ConnectError> {
        let conn = Connection::connect_to_env().map_err(ConnectError::WaylandConnectError)?;
        let mut wl = WlState::new();
        let mut queue = conn.new_event_queue::<WlState>();

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
            epoll: epoll::create(epoll::CreateFlags::CLOEXEC)
                .map_err(ConnectError::FailedToCreateEpoll)?,

            wl,
            queue,
            conn,
            readers: HashMap::new(),
            writers: HashMap::new(),
            mime_types: MimeTypes::new(),

            offer_seq: OfferSeq::Empty,
            source_to_text: HashMap::new(),
        })
    }

    fn handle(&mut self, event: WlEvent) -> Result<()> {
        match event {
            WlEvent::Offer(event) => self.handle_offer_event(event)?,
            WlEvent::Source(event) => self.handle_source_event(event)?,
        }

        Ok(())
    }

    fn handle_offer_event(&mut self, event: WlOfferEvent) -> Result<()> {
        match event {
            WlOfferEvent::DataOffer(offer) => {
                self.offer_seq.start(offer);
            }

            WlOfferEvent::MimeTime(offer, mime_type) => {
                self.offer_seq.extend(offer, mime_type);
            }

            WlOfferEvent::Selection(Some(offer)) => {
                let Some((offer, mime_types)) = self.offer_seq.finish(offer) else {
                    return Ok(());
                };
                let Some(mime_type_to_ask_for) = self.mime_types.choose(mime_types) else {
                    offer.destroy();
                    return Ok(());
                };

                match rustix::pipe::pipe_with(PipeFlags::NONBLOCK) {
                    Ok((reader, writer)) => {
                        offer.receive(mime_type_to_ask_for, writer.as_fd());
                        drop(writer);
                        self.add_reader(Reader::new(reader, offer))?;
                    }
                    Err(err) => {
                        log::error!("failed to create pipe: {err:?}");
                        offer.destroy();
                    }
                }
            }
            WlOfferEvent::Selection(None) => {
                self.offer_seq.destroy();
            }

            WlOfferEvent::PrimarySelection(offer) => {
                self.offer_seq.destroy();
                if let Some(offer) = offer {
                    offer.destroy();
                }
            }

            WlOfferEvent::Finished => {
                log::warn!("ExtDataControlDeviceV1 has finished");
                self.running = false;
            }
        }

        Ok(())
    }

    fn handle_source_event(&mut self, event: WlSourceEvent) -> Result<()> {
        match event {
            WlSourceEvent::Requested(source, mime_type, fd) => {
                if !MimeTypes::is_text(&mime_type) {
                    return Ok(());
                }
                let Some(text) = self.source_to_text.get(&source) else {
                    return Ok(());
                };

                match Writer::new(fd, text.clone()) {
                    Ok(writer) => self.add_writer(writer)?,
                    Err(err) => log::error!("{err:?}"),
                }
            }
            WlSourceEvent::Cancelled(source) => {
                self.source_to_text.remove(&source);
                source.destroy();
            }
        }
        Ok(())
    }

    fn cleanup(&mut self) {
        self.ext_data_control_device.destroy();
        self.wl_seat.release();
        self.ext_data_control_manager.destroy();

        for reader in self.readers.values() {
            reader.destroy();
        }
        self.offer_seq.destroy();
        for source in self.source_to_text.keys() {
            source.destroy()
        }

        if let Err(err) = self.queue.flush() {
            log::error!("failed to finish cleanup: {err:?}");
        }
    }

    fn add_reader(&mut self, reader: Reader) -> Result<()> {
        log::trace!("new reader {:?}", reader.as_raw_fd());
        epoll::add(
            &self.epoll,
            &reader,
            epoll::EventData::new_u64(reader.as_raw_fd() as u64),
            epoll::EventFlags::IN,
        )?;
        self.readers.insert(reader.as_raw_fd(), reader);
        Ok(())
    }

    fn add_writer(&mut self, writer: Writer) -> Result<()> {
        log::trace!("new writer {:?}", writer.as_raw_fd());
        epoll::add(
            &self.epoll,
            &writer,
            epoll::EventData::new_u64(writer.as_raw_fd() as u64),
            epoll::EventFlags::OUT,
        )?;
        self.writers.insert(writer.as_raw_fd(), writer);
        Ok(())
    }
}

fn main() -> Result<()> {
    env_logger::init();

    let mut state = State::connect()?;

    let source = state
        .ext_data_control_manager
        .create_data_source(&state.queue.handle(), ());
    source.offer("text/plain;charset=utf-8".to_string());
    source.offer("text/plain".to_string());
    source.offer(state.mime_types.mask().to_string());

    state.ext_data_control_device.set_selection(Some(&source));
    state.source_to_text.insert(source, String::from("FOO"));
    state.queue.flush()?;

    epoll::add(
        &state.epoll,
        state.conn.as_fd(),
        epoll::EventData::new_u64(state.conn.as_fd().as_raw_fd() as u64),
        epoll::EventFlags::IN,
    )?;
    let mut epoll_events = Vec::with_capacity(16);

    while state.running {
        state.queue.flush()?;
        state.queue.dispatch_pending(&mut state.wl)?;

        let wl_read_guard = state
            .queue
            .prepare_read()
            .context("failed to create ReadEventsGuard")?;

        log::trace!("epoll_wait()...");
        epoll::wait(&state.epoll, spare_capacity(&mut epoll_events), None)?;
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
                state.handle(event)?;
            }
        } else {
            drop(wl_read_guard);
        }

        use std::collections::hash_map::Entry;

        for fd in readers.dead {
            if let Entry::Occupied(entry) = state.readers.entry(fd) {
                let reader = entry.remove();
                epoll::delete(&state.epoll, &reader)?;
                reader.destroy();
            }
        }

        for fd in writers.dead {
            if let Entry::Occupied(entry) = state.writers.entry(fd) {
                let writer = entry.remove();
                epoll::delete(&state.epoll, &writer)?;
            }
        }

        for fd in readers.ready {
            if let Entry::Occupied(mut entry) = state.readers.entry(fd) {
                let reader = entry.get_mut();
                log::trace!("Reading {:?}", reader.as_raw_fd());
                match reader.read() {
                    Ok(ReadResult::Done(text)) => {
                        log::trace!("Got text {text:?}");
                        epoll::delete(&state.epoll, &mut *reader)?;
                        reader.destroy();
                        entry.remove();

                        if text == "EXIT" {
                            state.running = false;
                        }
                    }
                    Ok(ReadResult::Pending) => {}
                    Err(err) => {
                        log::error!("reader {:?} returned error {err:?}", reader.as_raw_fd());
                        epoll::delete(&state.epoll, &mut *reader)?;
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
                        epoll::delete(&state.epoll, writer)?;
                        entry.remove();
                    }
                    Ok(WriteResult::Pending) => {}
                    Err(err) => {
                        log::error!("writer {:?} returned error {err:?}", writer.as_raw_fd());
                        epoll::delete(&state.epoll, writer)?;
                        entry.remove();
                    }
                }
            }
        }
    }

    state.cleanup();

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
