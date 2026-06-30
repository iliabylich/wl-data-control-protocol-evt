use anyhow::{Context as _, Result, bail};
use rustix::event::{PollFd, PollFlags, poll};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    os::fd::{AsFd, AsRawFd, OwnedFd},
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

mod nonblocking;

mod reader;
use reader::{ReadResult, Reader};

struct Writer {
    fd: OwnedFd,
    buf: Vec<u8>,
}

struct Incoming {
    offer: ExtDataControlOfferV1,
    mime_types: HashSet<String>,
}

struct State {
    wl_seat: Option<WlSeat>,
    ext_data_control_manager: Option<ExtDataControlManagerV1>,
    ext_data_control_device: Option<ExtDataControlDeviceV1>,

    incoming: Option<Incoming>,

    // offer_to_mime_types: HashMap<ExtDataControlOfferV1, HashSet<String>>,
    source_to_text: HashMap<ExtDataControlSourceV1, String>,

    own_mime_type: String,

    readers_queue: VecDeque<Reader>,
    readers: HashMap<i32, Reader>,

    writers_queue: VecDeque<(OwnedFd, Vec<u8>)>,
    writers: HashMap<i32, Writer>,
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
            Event::Selection { id: Some(offer) } => {
                let incoming = state.incoming.take();

                if incoming.as_ref().is_some_and(|incoming| {
                    incoming.offer == offer
                        && incoming.mime_types.contains(TEXT_UTF8_MIME)
                        && !incoming.mime_types.contains(&state.own_mime_type)
                }) {
                    let (reader, writer) = rustix::pipe::pipe().unwrap();
                    offer.receive(String::from(TEXT_UTF8_MIME), writer.as_fd());
                    drop(writer);
                    state.readers_queue.push_back(Reader::new(reader, offer));
                } else {
                    offer.destroy();
                }
            }
            Event::PrimarySelection { id: Some(offer) } => {
                assert!(state.incoming.as_ref().is_some_and(|i| i.offer == offer));
                state.incoming = None;
                offer.destroy();
            }
            Event::DataOffer { id: offer } => {
                state.incoming = Some(Incoming {
                    offer,
                    mime_types: HashSet::new(),
                })
            }
            Event::Finished => {}

            Event::Selection { id: None } | Event::PrimarySelection { id: None } => {
                state.incoming = None;
            }

            event => todo!("unsuported ExtDataControlDeviceV1 event: {event:?}"),
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
                if let Some(incoming_offer) = &mut state.incoming {
                    assert_eq!(&incoming_offer.offer, proxy);
                    incoming_offer.mime_types.insert(mime_type);
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
                        state.writers_queue.push_back((fd, text.into_bytes()));
                    }
                }
            }
            Event::Cancelled => {
                state.source_to_text.remove(proxy);
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
        incoming: None,
        source_to_text: HashMap::new(),

        own_mime_type: format!("text/plain;sent-by-pid-{}", std::process::id()),

        readers: HashMap::new(),
        readers_queue: VecDeque::new(),

        writers: HashMap::new(),
        writers_queue: VecDeque::new(),
    };

    let display = conn.display();
    display.get_registry(&queue.handle(), ());
    queue.roundtrip(&mut state)?;
    if state.ext_data_control_manager.is_none() {
        bail!("Wayland protocol 'ext_data_control_v1' is not supported by your compositor");
    }

    // if let Some(ext_data_control_manager) = &state.ext_data_control_manager
    //     && let Some(ext_data_control_device) = &state.ext_data_control_device
    // {
    //     let source = ext_data_control_manager.create_data_source(&queue.handle(), ());
    //     source.offer("text/plain;charset=utf-8".to_string());
    //     source.offer("text/plain".to_string());
    //     source.offer(format!("text/plain;sent-by-pid-{}", std::process::id()));

    //     ext_data_control_device.set_selection(Some(&source));
    //     state.source_to_text.insert(source, String::from("FOO"));
    //     queue.flush()?;
    // }

    loop {
        println!("iteration");
        queue.flush()?;
        queue.dispatch_pending(&mut state)?;
        let wl_read_guard = queue
            .prepare_read()
            .context("failed to create ReadEventsGuard")?;

        let mut pollfds = state
            .readers
            .values()
            .map(Reader::as_pollfd)
            .chain([PollFd::new(&wl_fd, PollFlags::IN)])
            .collect::<Vec<_>>();
        // println!("{pollfds:?}");
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
        }

        for reader_fd in ready_reader_fds {
            if let Some(reader) = state.readers.get_mut(&reader_fd) {
                match reader.read() {
                    Ok(ReadResult::Done(text)) => {
                        println!("Got text {text:?}");
                        state.readers.remove(&reader_fd);
                    }
                    Ok(ReadResult::Pending) => {}
                    Err(err) => {
                        println!("reader {:?} returned error {err:?}", reader.as_raw_fd());
                        state.readers.remove(&reader_fd);
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
    }
}

#[derive(Debug)]
enum REvents {
    IN,
    OUT,
}
impl REvents {
    fn new(fd: i32, revents: PollFlags) -> Result<Option<Self>> {
        if revents.intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL) {
            bail!("FD {fd} returned revents {revents:?}");
        } else if revents.contains(PollFlags::IN) {
            Ok(Some(Self::IN))
        } else if revents.contains(PollFlags::OUT) {
            Ok(Some(Self::OUT))
        } else {
            Ok(None)
        }
    }
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

    for (fd, revents) in pollfds {
        if *fd == wl_fd {
            if revents.intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL) {
                bail!("Wayland returned revents {revents:?}");
            } else if revents.contains(PollFlags::IN) {
                wl_is_readable = true;
            }
        } else if readers.contains_key(fd) {
            if revents.intersects(PollFlags::ERR | PollFlags::NVAL) {
                println!("Reader with FD {fd} returned revents {revents:?}, removing it");
                readers.remove(fd).unwrap();
            } else if revents.intersects(PollFlags::IN | PollFlags::HUP) {
                readable_readers.push(*fd);
            }
        } else if writers.contains_key(fd) {
            if revents.intersects(PollFlags::ERR | PollFlags::NVAL) {
                println!("Writer with FD {fd} returned revents {revents:?}, removing it");
                writers.remove(fd).unwrap();
            } else if revents.contains(PollFlags::IN) {
                writable_writers.push(*fd);
            }
        }
    }

    Ok((wl_is_readable, readable_readers, writable_writers))
}
