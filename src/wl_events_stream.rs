use crate::{
    app_connection::AppConnection,
    wl_event::{WlEvent, WlRegistryEvent},
};
use crossbeam_queue::SegQueue;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child,
    protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::ExtDataControlOfferV1,
    ext_data_control_source_v1::ExtDataControlSourceV1,
};

pub(crate) struct WlEventsStream;

impl Dispatch<WlRegistry, &SegQueue<WlRegistryEvent>> for WlEventsStream {
    fn event(
        _state: &mut Self,
        _registry: &WlRegistry,
        event: <WlRegistry as Proxy>::Event,
        events: &&SegQueue<WlRegistryEvent>,
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

impl Dispatch<ExtDataControlDeviceV1, &SegQueue<WlEvent>> for WlEventsStream {
    fn event(
        _state: &mut Self,
        _proxy: &ExtDataControlDeviceV1,
        event: <ExtDataControlDeviceV1 as Proxy>::Event,
        events: &&SegQueue<WlEvent>,
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
                AppConnection::wl_events_queue()
            )
        ]
    );
}

impl Dispatch<ExtDataControlOfferV1, &SegQueue<WlEvent>> for WlEventsStream {
    fn event(
        _state: &mut Self,
        proxy: &ExtDataControlOfferV1,
        event: <ExtDataControlOfferV1 as Proxy>::Event,
        events: &&SegQueue<WlEvent>,
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

impl Dispatch<ExtDataControlSourceV1, &SegQueue<WlEvent>> for WlEventsStream {
    fn event(
        _self: &mut Self,
        proxy: &ExtDataControlSourceV1,
        event: <ExtDataControlSourceV1 as Proxy>::Event,
        events: &&SegQueue<WlEvent>,
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
