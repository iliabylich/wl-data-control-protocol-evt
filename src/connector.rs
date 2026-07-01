use crate::{wl_event::WlRegistryEvent, wl_state::WlState};
use wayland_client::{Connection, EventQueue, protocol::wl_seat::WlSeat};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::ExtDataControlDeviceV1,
    ext_data_control_manager_v1::ExtDataControlManagerV1,
};

pub(crate) struct Connector;

pub(crate) struct ConnectorOutput {
    pub(crate) conn: Connection,
    pub(crate) wl: WlState,
    pub(crate) queue: EventQueue<WlState>,

    pub(crate) wl_seat: WlSeat,
    pub(crate) ext_data_control_manager: ExtDataControlManagerV1,
    pub(crate) ext_data_control_device: ExtDataControlDeviceV1,
}

impl Connector {
    pub(crate) fn connect() -> Result<ConnectorOutput, ConnectorError> {
        let conn = Connection::connect_to_env()?;
        let mut queue = conn.new_event_queue::<WlState>();
        let mut wl = WlState::new();

        let (wl_seat, ext_data_control_manager, ext_data_control_device) =
            get_objects(&conn, &mut queue, &mut wl)?;

        Ok(ConnectorOutput {
            conn,
            wl,
            queue,

            wl_seat,
            ext_data_control_manager,
            ext_data_control_device,
        })
    }
}

pub(crate) fn get_objects(
    conn: &Connection,
    queue: &mut EventQueue<WlState>,
    wl: &mut WlState,
) -> Result<(WlSeat, ExtDataControlManagerV1, ExtDataControlDeviceV1), ConnectorError> {
    let registry = conn.display().get_registry(&queue.handle(), ());
    queue.roundtrip(wl)?;

    let mut wl_seat: Option<WlSeat> = None;
    let mut ext_data_control_manager: Option<ExtDataControlManagerV1> = None;

    while let Some(event) = wl.registry_events.pop_front() {
        match event {
            WlRegistryEvent::WlSeat { name, version } => {
                wl_seat = Some(registry.bind(name, version, &queue.handle(), ()));
            }
            WlRegistryEvent::ExtDataControlManager { name, version } => {
                ext_data_control_manager = Some(registry.bind(name, version, &queue.handle(), ()));
            }
            WlRegistryEvent::Other => {}
        }
    }

    let wl_seat = wl_seat.ok_or(ConnectorError::NoSeat)?;
    let ext_data_control_manager = ext_data_control_manager.ok_or(ConnectorError::Unsupported)?;

    let ext_data_control_device =
        ext_data_control_manager.get_data_device(&wl_seat, &queue.handle(), ());

    Ok((wl_seat, ext_data_control_manager, ext_data_control_device))
}

#[derive(Debug)]
pub(crate) enum ConnectorError {
    WaylandConnectFailed(wayland_client::ConnectError),
    WaylandDispatchFailed(wayland_client::DispatchError),
    NoSeat,
    Unsupported,
}

impl From<wayland_client::ConnectError> for ConnectorError {
    fn from(err: wayland_client::ConnectError) -> Self {
        Self::WaylandConnectFailed(err)
    }
}

impl From<wayland_client::DispatchError> for ConnectorError {
    fn from(err: wayland_client::DispatchError) -> Self {
        Self::WaylandDispatchFailed(err)
    }
}

impl core::fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WaylandConnectFailed(err) => write!(f, "WaylandConnectError({err})"),
            Self::WaylandDispatchFailed(err) => write!(f, "WaylandDispatchError({err})"),
            Self::NoSeat => write!(f, "NoSeat"),
            Self::Unsupported => write!(f, "Unsupported"),
        }
    }
}

impl core::error::Error for ConnectorError {}
