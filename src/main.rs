use anyhow::{Context as _, Result, bail};
use rustix::{
    event::{PollFd, PollFlags, poll},
    pipe::PipeFlags,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    os::fd::{AsFd, AsRawFd},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child,
    protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
    ext_data_control_source_v1::ExtDataControlSourceV1,
};

mod reader;
use reader::{ReadResult, Reader};

mod writer;
use writer::{WriteResult, Writer};

enum Incoming {
    Some {
        offer: ExtDataControlOfferV1,
        mimes: HashSet<String>,
    },
    None,
}
enum IncomingMatch {
    Matched(ExtDataControlOfferV1, HashSet<String>),
    Mismatch(ExtDataControlOfferV1, ExtDataControlOfferV1),
    NoMatch(ExtDataControlOfferV1),
}
enum AddMimeError {
    OfferMismatch,
    NoOffer,
}
impl Incoming {
    pub(crate) fn start(&mut self, offer: ExtDataControlOfferV1) {
        *self = Self::Some {
            offer,
            mimes: HashSet::new(),
        }
    }
    pub(crate) fn add_mime(
        &mut self,
        offer: &ExtDataControlOfferV1,
        mime: String,
    ) -> Result<(), AddMimeError> {
        if let Self::Some {
            mimes,
            offer: offer_,
        } = self
        {
            if offer_ != offer {
                return Err(AddMimeError::OfferMismatch);
            }
            mimes.insert(mime);
            Ok(())
        } else {
            Err(AddMimeError::NoOffer)
        }
    }
    pub(crate) fn take(&mut self) -> Self {
        let mut this = Self::None;
        core::mem::swap(self, &mut this);
        this
    }
    pub(crate) fn match_on(self, next: ExtDataControlOfferV1) -> IncomingMatch {
        match self {
            Incoming::Some { offer: prev, mimes } if prev == next => {
                IncomingMatch::Matched(prev, mimes)
            }
            Incoming::Some { offer: prev, .. } => IncomingMatch::Mismatch(prev, next),
            Incoming::None => IncomingMatch::NoMatch(next),
        }
    }
    pub(crate) fn destroy_if_present(&mut self) {
        if let Incoming::Some { offer, .. } = self.take() {
            offer.destroy();
        }
    }
}

struct State {
    wl_seat: Option<WlSeat>,
    ext_data_control_manager: Option<ExtDataControlManagerV1>,
    ext_data_control_device: Option<ExtDataControlDeviceV1>,

    incoming: Incoming,

    source_to_text: HashMap<ExtDataControlSourceV1, String>,

    mime_mask: String,

    readers_queue: VecDeque<Reader>,
    readers: HashMap<i32, Reader>,

    writers_queue: VecDeque<Writer>,
    writers: HashMap<i32, Writer>,

    cancelled: bool,
}

impl State {
    fn schedule_cleanup(&mut self) {
        if let Some(wl_seat) = &self.wl_seat {
            wl_seat.release();
        }
        self.wl_seat = None;

        // --

        if let Some(ext_data_control_manager) = &self.ext_data_control_manager {
            ext_data_control_manager.destroy();
        }
        self.ext_data_control_manager = None;

        // --

        if let Some(ext_data_control_device) = &self.ext_data_control_device {
            ext_data_control_device.destroy();
        }
        self.ext_data_control_device = None;

        // --

        self.incoming.destroy_if_present();

        // --

        for source in self.source_to_text.keys() {
            source.destroy()
        }
        self.source_to_text.clear();

        // --

        for reader in &self.readers_queue {
            reader.destroy();
        }
        self.readers_queue.clear();

        // --

        for reader in self.readers.values() {
            reader.destroy();
        }
        self.readers.clear();

        // --

        self.writers.clear();
        self.writers_queue.clear();
    }
}

const TEXT_MIME: &str = "text/plain";
const TEXT_UTF8_MIME: &str = "text/plain;charset=utf-8";

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: <WlRegistry as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wayland_client::protocol::wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };

        if interface == WlSeat::interface().name {
            println!("Got wl_seat");
            state.wl_seat = Some(registry.bind(name, version, qh, ()));
        } else if interface == ExtDataControlManagerV1::interface().name {
            println!("Got ext_data_control_manager_v1");
            state.ext_data_control_manager = Some(registry.bind(name, version, qh, ()));
        } else {
            return;
        }

        if let Some(wl_seat) = &state.wl_seat
            && let Some(wl_data_control_manager) = &state.ext_data_control_manager
            && state.ext_data_control_device.is_none()
        {
            state.ext_data_control_device =
                Some(wl_data_control_manager.get_data_device(wl_seat, qh, ()));
            println!("Got ext_data_control_device");
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlSeat,
        _event: <WlSeat as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlManagerV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ExtDataControlManagerV1,
        _event: <ExtDataControlManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &ExtDataControlDeviceV1,
        event: <ExtDataControlDeviceV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1::Event;

        match event {
            Event::Selection { id: Some(offer) } => match state.incoming.take().match_on(offer) {
                IncomingMatch::Matched(offer, mimes) => {
                    let mime_to_request = if mimes.contains(&state.mime_mask) {
                        None
                    } else if mimes.contains(TEXT_UTF8_MIME) {
                        Some(TEXT_UTF8_MIME)
                    } else if mimes.contains(TEXT_MIME) {
                        Some(TEXT_MIME)
                    } else {
                        None
                    };

                    if let Some(mime) = mime_to_request {
                        match rustix::pipe::pipe_with(PipeFlags::NONBLOCK) {
                            Ok((reader, writer)) => {
                                offer.receive(String::from(mime), writer.as_fd());
                                drop(writer);
                                state.readers_queue.push_back(Reader::new(reader, offer));
                            }
                            Err(err) => {
                                eprintln!("failed to create pipe: {err:?}");
                                offer.destroy();
                            }
                        }
                    } else {
                        offer.destroy();
                    }
                }
                IncomingMatch::Mismatch(prev, next) => {
                    eprintln!("Something is wrong with the sequence of events");
                    eprintln!("Selection->offer doesn't match state->incoming");
                    prev.destroy();
                    next.destroy();
                }
                IncomingMatch::NoMatch(offer) => {
                    offer.destroy();
                }
            },
            Event::PrimarySelection { id: Some(offer) } => {
                match state.incoming.take().match_on(offer) {
                    IncomingMatch::Matched(same, _) => {
                        same.destroy();
                    }
                    IncomingMatch::Mismatch(prev, next) => {
                        eprintln!("Something is wrong with the sequence of events");
                        eprintln!("PrimarySelection->offer doesn't match state->incoming");
                        prev.destroy();
                        next.destroy();
                    }
                    IncomingMatch::NoMatch(offer) => {
                        offer.destroy();
                    }
                }
            }
            Event::DataOffer { id: offer } => {
                state.incoming.destroy_if_present();
                state.incoming.start(offer);
            }
            Event::Finished => {
                eprintln!("ExtDataControlDeviceV1 has finished");
                state.cancelled = true;
            }

            Event::Selection { id: None } | Event::PrimarySelection { id: None } => {
                state.incoming.destroy_if_present();
            }

            event => unreachable!("unsuported ExtDataControlDeviceV1 event: {event:?}"),
        }
    }

    event_created_child!(State,
        ExtDataControlDeviceV1, [
            ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (
                ExtDataControlOfferV1,
                ()
            )
        ]
    );
}

impl Dispatch<ExtDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ExtDataControlOfferV1,
        event: <ExtDataControlOfferV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_offer_v1::Event::Offer { mime_type } => {
                if let Err(err) = state.incoming.add_mime(proxy, mime_type) {
                    match err {
                        AddMimeError::OfferMismatch => {
                            eprintln!("Wrong sequence of events");
                            eprintln!("Got mime type offer for a different offer");
                            state.incoming.destroy_if_present();
                        }
                        AddMimeError::NoOffer => {
                            eprintln!("Wrong sequence of events");
                            eprintln!("Got mime typer offer before receiing a data offer");
                            state.incoming.destroy_if_present();
                        }
                    }
                }
            }
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ExtDataControlSourceV1,
        event: <ExtDataControlSourceV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1::Event;

        match event {
            Event::Send { mime_type, fd } => {
                if mime_type == TEXT_UTF8_MIME || mime_type == TEXT_MIME {
                    if let Some(text) = state.source_to_text.get(proxy).cloned() {
                        match Writer::new(fd, text) {
                            Ok(writer) => state.writers_queue.push_back(writer),
                            Err(err) => eprintln!("{err:?}"),
                        }
                    }
                }
            }
            Event::Cancelled => {
                state.source_to_text.remove(proxy);
                proxy.destroy();
            }
            _ => todo!(),
        }
    }
}

fn main() -> Result<()> {
    let conn = Connection::connect_to_env()?;
    let wl_fd = conn.as_fd();

    let mut queue = conn.new_event_queue::<State>();
    let mut state = State {
        wl_seat: None,
        ext_data_control_manager: None,
        ext_data_control_device: None,
        incoming: Incoming::None,
        source_to_text: HashMap::new(),

        mime_mask: format!(
            "application/x-wayland-clipboard-poll-pid-{}",
            std::process::id()
        ),

        readers: HashMap::new(),
        readers_queue: VecDeque::new(),

        writers: HashMap::new(),
        writers_queue: VecDeque::new(),

        cancelled: false,
    };

    let display = conn.display();
    display.get_registry(&queue.handle(), ());
    queue.roundtrip(&mut state)?;
    if state.wl_seat.is_none() {
        bail!("failed to get wl_seat");
    }
    if state.ext_data_control_manager.is_none() || state.ext_data_control_device.is_none() {
        bail!("Wayland protocol 'ext_data_control_v1' is not supported by your compositor");
    }

    if let Some(ext_data_control_manager) = &state.ext_data_control_manager
        && let Some(ext_data_control_device) = &state.ext_data_control_device
    {
        let source = ext_data_control_manager.create_data_source(&queue.handle(), ());
        source.offer("text/plain;charset=utf-8".to_string());
        source.offer("text/plain".to_string());
        source.offer(state.mime_mask.clone());

        ext_data_control_device.set_selection(Some(&source));
        state.source_to_text.insert(source, String::from("FOO"));
        queue.flush()?;
    }

    while !state.cancelled {
        println!("iteration");
        queue.flush()?;
        queue.dispatch_pending(&mut state)?;
        let wl_read_guard = queue
            .prepare_read()
            .context("failed to create ReadEventsGuard")?;

        let mut pollfds = core::iter::empty()
            .chain(state.readers.values().map(Reader::as_pollfd))
            .chain(state.writers.values().map(Writer::as_pollfd))
            .chain([PollFd::new(&wl_fd, PollFlags::IN)])
            .collect::<Vec<_>>();
        poll(&mut pollfds, None)?;
        let fd_to_revents = pollfds
            .into_iter()
            .map(|pollfd| (pollfd.as_fd().as_raw_fd(), pollfd.revents()))
            .collect::<Vec<_>>();
        let (wl_is_readable, ready_reader_fds, ready_writer_fds) = classify_pollfds(
            &fd_to_revents,
            wl_fd.as_raw_fd(),
            &mut state.readers,
            &mut state.writers,
        )?;

        if wl_is_readable {
            wl_read_guard.read()?;
            queue.dispatch_pending(&mut state)?;
        } else {
            drop(wl_read_guard);
        }

        use std::collections::hash_map::Entry;

        for reader_fd in ready_reader_fds {
            if let Entry::Occupied(mut entry) = state.readers.entry(reader_fd) {
                let reader = entry.get_mut();
                match reader.read() {
                    Ok(ReadResult::Done(text)) => {
                        println!("Got text {text:?}");
                        reader.destroy();
                        entry.remove();

                        if text == "EXIT" {
                            state.cancelled = true;
                        }
                    }
                    Ok(ReadResult::Pending) => {}
                    Err(err) => {
                        println!("reader {:?} returned error {err:?}", reader.as_raw_fd());
                        reader.destroy();
                        entry.remove();
                    }
                }
            }
        }

        for writer_fd in ready_writer_fds {
            if let Entry::Occupied(mut entry) = state.writers.entry(writer_fd) {
                let writer = entry.get_mut();
                println!("Writing {:?}", writer.as_raw_fd());
                match writer.write() {
                    Ok(WriteResult::Done) => {
                        println!("Done writing to {:?}", writer.as_raw_fd());
                        entry.remove();
                    }
                    Ok(WriteResult::Pending) => {}
                    Err(err) => {
                        println!("writer {:?} returned error {err:?}", writer.as_raw_fd());
                        entry.remove();
                    }
                }
            }
        }

        if !state.readers_queue.is_empty() {
            queue.flush()?;
        }
        while let Some(reader) = state.readers_queue.pop_front() {
            println!("Got new reader, adding to staate {:?}", reader.as_raw_fd());
            state.readers.insert(reader.as_raw_fd(), reader);
        }
        while let Some(writer) = state.writers_queue.pop_front() {
            println!("Got new writer, adding to staate {:?}", writer.as_raw_fd());
            state.writers.insert(writer.as_raw_fd(), writer);
        }
    }

    state.schedule_cleanup();
    queue.flush()?;

    Ok(())
}

fn classify_pollfds(
    pollfds: &[(i32, PollFlags)],
    wl_fd: i32,
    readers: &mut HashMap<i32, Reader>,
    writers: &mut HashMap<i32, Writer>,
) -> Result<(bool, Vec<i32>, Vec<i32>)> {
    let mut wl_is_readable = false;
    let mut readable_readers = vec![];
    let mut writable_writers = vec![];

    use std::collections::hash_map::Entry;

    for (fd, revents) in pollfds {
        if *fd == wl_fd {
            if revents.intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL) {
                bail!("Wayland returned revents {revents:?}");
            } else if revents.contains(PollFlags::IN) {
                wl_is_readable = true;
            }
        } else if let Entry::Occupied(entry) = readers.entry(*fd) {
            if revents.intersects(PollFlags::ERR | PollFlags::NVAL) {
                println!("Reader with FD {fd} returned revents {revents:?}, removing it");
                let reader = entry.remove();
                reader.destroy();
            } else if revents.intersects(PollFlags::IN | PollFlags::HUP) {
                readable_readers.push(*fd);
            }
        } else if let Entry::Occupied(entry) = writers.entry(*fd) {
            if revents.intersects(PollFlags::ERR | PollFlags::NVAL | PollFlags::HUP) {
                println!("Writer with FD {fd} returned revents {revents:?}, removing it");
                entry.remove();
            } else if revents.contains(PollFlags::OUT) {
                writable_writers.push(*fd);
            }
        }
    }

    Ok((wl_is_readable, readable_readers, writable_writers))
}
