use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::Read,
    os::fd::{AsFd, OwnedFd},
};

use anyhow::{Context as _, Result, bail};
use rustix::event::{PollFd, PollFlags, poll};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child,
    protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
};

struct CopiedText {
    reader: OwnedFd,
}

struct State {
    wl_seat: Option<WlSeat>,
    ext_data_control_manager: Option<ExtDataControlManagerV1>,
    ext_data_control_device: Option<ExtDataControlDeviceV1>,
    offer_to_mime_types: HashMap<ExtDataControlOfferV1, Vec<String>>,
    copied: VecDeque<CopiedText>,
}

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

        if interface == "wl_seat" {
            println!("Got wl_seat");
            state.wl_seat = Some(registry.bind(name, version, qh, ()));
        } else if interface == "ext_data_control_manager_v1" {
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
        const TEXT_MIME: &str = "text/plain;charset=utf-8";

        match event {
            Event::Selection { id: Some(offer) } => {
                let Some(mime_types) = state.offer_to_mime_types.remove(&offer) else {
                    offer.destroy();
                    return;
                };
                let is_text = mime_types.iter().any(|m| m == TEXT_MIME);
                if !is_text {
                    offer.destroy();
                    return;
                }
                let (reader, writer) = rustix::pipe::pipe().unwrap();
                offer.receive(String::from(TEXT_MIME), writer.as_fd());
                drop(writer);

                state.copied.push_back(CopiedText { reader });
                offer.destroy();
            }
            Event::PrimarySelection { id: Some(offer) } => {
                state.offer_to_mime_types.remove(&offer);
                offer.destroy();
            }

            _ => {}
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
                state
                    .offer_to_mime_types
                    .entry(proxy.clone())
                    .or_default()
                    .push(mime_type);
            }
            _ => unreachable!(),
        }
    }
}

fn main() -> Result<()> {
    let conn = Connection::connect_to_env()?;

    let mut queue = conn.new_event_queue::<State>();
    let mut state = State {
        wl_seat: None,
        ext_data_control_manager: None,
        ext_data_control_device: None,
        offer_to_mime_types: HashMap::new(),
        copied: VecDeque::new(),
    };

    let display = conn.display();
    display.get_registry(&queue.handle(), ());
    queue.roundtrip(&mut state)?;
    if state.ext_data_control_manager.is_none() {
        bail!("Wayland protocol 'ext_data_control_v1' is not supported by your compositor");
    }

    loop {
        queue.flush()?;
        queue.dispatch_pending(&mut state)?;
        let read_guard = queue.prepare_read().context("failed to get ReadGuard")?;
        let fd = read_guard.connection_fd();

        let mut pollfds = [PollFd::new(&fd, PollFlags::IN)];
        poll(&mut pollfds, None)?;
        let revents = pollfds[0].revents();
        if revents.intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL) {
            bail!("FD {fd:?} returned revents {revents:?}");
        } else {
            assert!(revents.contains(PollFlags::IN))
        }
        read_guard.read()?;
        queue.dispatch_pending(&mut state)?;

        if !state.copied.is_empty() {
            queue.flush()?;
        }

        while let Some(CopiedText { reader }) = state.copied.pop_front() {
            println!("Reading copied text...");
            let mut f = File::from(reader);
            let mut text = String::new();
            f.read_to_string(&mut text)?;
            println!("Copied text: {text:?}");
        }
    }
}
