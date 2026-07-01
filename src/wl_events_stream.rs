use crate::wl_event::{WlEvent, WlRegistryEvent};
use crossbeam_queue::SegQueue;
use std::sync::Arc;
use wayland_client::{
    Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle,
    backend::WaylandError,
    event_created_child,
    protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::ExtDataControlOfferV1,
    ext_data_control_source_v1::ExtDataControlSourceV1,
};

pub(crate) struct WlEventsStream {
    events: Vec<WlEvent>,
}

impl WlEventsStream {
    pub(crate) fn new() -> Self {
        Self { events: vec![] }
    }

    pub(crate) fn get_registry_and_registry_events_sync(
        conn: &Connection,
        queue: &mut EventQueue<Self>,
    ) -> Result<(WlRegistry, Vec<WlRegistryEvent>), DispatchError> {
        let buffer: Arc<SegQueue<WlRegistryEvent>> = Arc::new(SegQueue::new());
        let registry = conn
            .display()
            .get_registry(&queue.handle(), Arc::clone(&buffer));
        let mut this = Self { events: vec![] };
        queue.roundtrip(&mut this)?;

        let mut events = vec![];
        while let Some(event) = buffer.pop() {
            events.push(event);
        }
        Ok((registry, events))
    }

    pub(crate) fn read_until_blocked(
        &mut self,
        queue: &mut EventQueue<Self>,
    ) -> Result<Vec<WlEvent>, WlEventStreamReadError> {
        let wl_read_guard = queue
            .prepare_read()
            .expect("failed to create ReadEventsGuard");
        wl_read_guard.read()?;
        queue.dispatch_pending(self)?;

        let events = core::mem::take(&mut self.events);
        Ok(events)
    }
}

#[derive(Debug)]
pub(crate) enum WlEventStreamReadError {
    Wayland(WaylandError),
    Dispatch(DispatchError),
}

impl core::fmt::Display for WlEventStreamReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wayland(err) => write!(f, "Wayland({err})"),
            Self::Dispatch(err) => write!(f, "Dispatch({err})"),
        }
    }
}

impl core::error::Error for WlEventStreamReadError {}

impl From<WaylandError> for WlEventStreamReadError {
    fn from(err: WaylandError) -> Self {
        Self::Wayland(err)
    }
}

impl From<DispatchError> for WlEventStreamReadError {
    fn from(err: DispatchError) -> Self {
        Self::Dispatch(err)
    }
}

impl Dispatch<WlRegistry, Arc<SegQueue<WlRegistryEvent>>> for WlEventsStream {
    fn event(
        _state: &mut Self,
        _registry: &WlRegistry,
        event: <WlRegistry as Proxy>::Event,
        events: &Arc<SegQueue<WlRegistryEvent>>,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match WlRegistryEvent::try_from(event) {
            Ok(event) => events.push(event),

            Err(unknown) => {
                log::error!("unknown WlRegistry event: {unknown:?}")
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for WlEventsStream {
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

impl Dispatch<ExtDataControlManagerV1, ()> for WlEventsStream {
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

impl Dispatch<ExtDataControlDeviceV1, ()> for WlEventsStream {
    fn event(
        Self { events }: &mut Self,
        _proxy: &ExtDataControlDeviceV1,
        event: <ExtDataControlDeviceV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match WlEvent::try_from(event) {
            Ok(wl_event) => events.push(wl_event),

            Err(unknown) => {
                log::error!("unknown ExtDataControlDeviceV1 event: {unknown:?}")
            }
        }
    }

    event_created_child!(WlEventsStream,
        ExtDataControlDeviceV1, [
            ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (
                ExtDataControlOfferV1,
                ()
            )
        ]
    );
}

impl Dispatch<ExtDataControlOfferV1, ()> for WlEventsStream {
    fn event(
        Self { events }: &mut Self,
        proxy: &ExtDataControlOfferV1,
        event: <ExtDataControlOfferV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match WlEvent::try_from((proxy.clone(), event)) {
            Ok(wl_event) => events.push(wl_event),

            Err(unknown) => {
                log::error!("unknown ExtDataControlOfferV1 event: {unknown:?}")
            }
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for WlEventsStream {
    fn event(
        Self { events }: &mut Self,
        proxy: &ExtDataControlSourceV1,
        event: <ExtDataControlSourceV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match WlEvent::try_from((proxy.clone(), event)) {
            Ok(wl_event) => events.push(wl_event),

            Err(unknown) => {
                log::error!("unknown ExtDataControlSourceV1 event: {unknown:?}")
            }
        }
    }
}
