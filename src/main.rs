use rustix::pipe::PipeFlags;
use std::{
    collections::HashMap,
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
use offer_seq::OfferSeq;

mod wl_event;
use wl_event::{WlEvent, WlOfferEvent, WlRegistryEvent, WlSourceEvent};

mod wl_state;
use wl_state::WlState;

mod mime_types;
use mime_types::MimeTypes;

mod epoll;
use epoll::{Epoll, EpollError, EpollResult};

struct State {
    wl_seat: WlSeat,
    ext_data_control_manager: ExtDataControlManagerV1,
    ext_data_control_device: ExtDataControlDeviceV1,

    epoll: Epoll,
    running: bool,

    _connection: Connection,
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
    EpollError(EpollError),
}

impl From<EpollError> for ConnectError {
    fn from(err: EpollError) -> Self {
        Self::EpollError(err)
    }
}

impl From<wayland_client::ConnectError> for ConnectError {
    fn from(err: wayland_client::ConnectError) -> Self {
        Self::WaylandConnectError(err)
    }
}

impl From<wayland_client::DispatchError> for ConnectError {
    fn from(err: wayland_client::DispatchError) -> Self {
        Self::WaylandDispatchError(err)
    }
}

impl core::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WaylandConnectError(err) => write!(f, "WaylandConnectError({err})"),
            Self::WaylandDispatchError(err) => write!(f, "WaylandDispatchError({err})"),
            Self::NoSeat => write!(f, "NoSeat"),
            Self::Unsupported => write!(f, "Unsupported"),
            Self::EpollError(err) => write!(f, "EpollError({err})"),
        }
    }
}

impl core::error::Error for ConnectError {}

impl State {
    fn connect() -> Result<Self, ConnectError> {
        let conn = Connection::connect_to_env()?;
        let mut wl = WlState::new();
        let mut queue = conn.new_event_queue::<WlState>();

        let registry = conn.display().get_registry(&queue.handle(), ());
        queue.roundtrip(&mut wl)?;

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
            epoll: Epoll::new(&conn)?,

            wl,
            queue,
            _connection: conn,
            readers: HashMap::new(),
            writers: HashMap::new(),
            mime_types: MimeTypes::new(),

            offer_seq: OfferSeq::Empty,
            source_to_text: HashMap::new(),
        })
    }

    fn handle(&mut self, event: WlEvent) -> Result<(), EpollError> {
        match event {
            WlEvent::Offer(event) => self.handle_offer_event(event)?,
            WlEvent::Source(event) => self.handle_source_event(event)?,
        }

        Ok(())
    }

    fn handle_offer_event(&mut self, event: WlOfferEvent) -> Result<(), EpollError> {
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

    fn handle_source_event(&mut self, event: WlSourceEvent) -> Result<(), EpollError> {
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

    fn add_reader(&mut self, reader: Reader) -> Result<(), EpollError> {
        log::trace!("new reader {:?}", reader.as_raw_fd());
        self.epoll.add_reader(&reader)?;
        self.readers.insert(reader.as_raw_fd(), reader);
        Ok(())
    }

    fn remove_reader(&mut self, fd: i32) -> Result<(), EpollError> {
        let Some(reader) = self.readers.remove(&fd) else {
            return Ok(());
        };
        self.epoll.delete(&reader)?;
        reader.destroy();
        Ok(())
    }

    fn add_writer(&mut self, writer: Writer) -> Result<(), EpollError> {
        log::trace!("new writer {:?}", writer.as_raw_fd());
        self.epoll.add_writer(&writer)?;
        self.writers.insert(writer.as_raw_fd(), writer);
        Ok(())
    }

    fn remove_writer(&mut self, fd: i32) -> Result<(), EpollError> {
        let Some(writer) = self.writers.remove(&fd) else {
            return Ok(());
        };
        self.epoll.delete(&writer)?;
        Ok(())
    }

    fn offer_text(&mut self, text: String) -> Result<(), wayland_client::backend::WaylandError> {
        let source = self
            .ext_data_control_manager
            .create_data_source(&self.queue.handle(), ());
        source.offer("text/plain;charset=utf-8".to_string());
        source.offer("text/plain".to_string());
        source.offer(self.mime_types.mask().to_string());

        self.ext_data_control_device.set_selection(Some(&source));
        self.source_to_text.insert(source, text);
        self.queue.flush()?;
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut state = State::connect()?;
    state.offer_text(String::from("BOO"))?;

    let mut epoll_events = Vec::with_capacity(16);

    while state.running {
        state.queue.flush()?;
        state.queue.dispatch_pending(&mut state.wl)?;

        let wl_read_guard = state
            .queue
            .prepare_read()
            .expect("failed to create ReadEventsGuard");

        let EpollResult {
            wl_is_readable,
            readers,
            writers,
        } = state
            .epoll
            .wait(&mut epoll_events, None, &state.readers, &state.writers)?;
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

        for fd in readers.dead {
            state.remove_reader(fd)?;
        }

        for fd in writers.dead {
            state.remove_writer(fd)?;
        }

        for fd in readers.ready {
            if let Some(reader) = state.readers.get_mut(&fd) {
                log::trace!("Reading {fd:?}");
                match reader.read() {
                    Ok(ReadResult::Done(text)) => {
                        log::trace!("Got text {text:?}");
                        state.remove_reader(fd)?;

                        if text == "EXIT" {
                            state.running = false;
                        }
                    }
                    Ok(ReadResult::Pending) => {}
                    Err(err) => {
                        log::error!("reader {fd:?} returned error {err:?}");
                        state.remove_reader(fd)?;
                    }
                }
            }
        }

        for fd in writers.ready {
            if let Some(writer) = state.writers.get_mut(&fd) {
                log::trace!("Writing {fd:?}");
                match writer.write() {
                    Ok(WriteResult::Done) => {
                        log::trace!("Done writing to {fd:?}");
                        state.remove_writer(fd)?;
                    }
                    Ok(WriteResult::Pending) => {}
                    Err(err) => {
                        log::error!("writer {fd:?} returned error {err:?}");
                        state.remove_writer(fd)?;
                    }
                }
            }
        }
    }

    state.cleanup();

    Ok(())
}
