use crate::{wl_event::WlRegistryEvent, wl_events_stream::WlEventsStream};
use std::os::fd::{AsFd, AsRawFd};
use wayland_client::{
    Connection as WlConnection, EventQueue, backend::WaylandError, protocol::wl_seat::WlSeat,
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::ExtDataControlDeviceV1,
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_source_v1::ExtDataControlSourceV1,
};

pub(crate) struct Connection {
    pub(crate) conn: WlConnection,
    pub(crate) queue: EventQueue<WlEventsStream>,

    pub(crate) wl_seat: WlSeat,
    pub(crate) ext_data_control_manager: ExtDataControlManagerV1,
    pub(crate) ext_data_control_device: ExtDataControlDeviceV1,
}

impl Connection {
    pub(crate) fn connect() -> Result<Self, ConnectorError> {
        let conn = WlConnection::connect_to_env()?;
        let mut queue = conn.new_event_queue::<WlEventsStream>();

        let (wl_seat, ext_data_control_manager, ext_data_control_device) =
            get_objects(&conn, &mut queue)?;

        Ok(Self {
            conn,
            queue,

            wl_seat,
            ext_data_control_manager,
            ext_data_control_device,
        })
    }

    pub(crate) fn cleanup_and_flush(&self) {
        self.ext_data_control_device.destroy();
        self.wl_seat.release();
        self.ext_data_control_manager.destroy();

        if let Err(err) = self.queue.flush() {
            log::error!("failed to finish cleanup: {err:?}");
        }
    }

    pub(crate) fn offer_text(
        &self,
        custom_mime_type: impl Into<String>,
    ) -> Result<ExtDataControlSourceV1, WaylandError> {
        let source = self
            .ext_data_control_manager
            .create_data_source(&self.queue.handle(), ());
        source.offer("text/plain;charset=utf-8".to_string());
        source.offer("text/plain".to_string());
        source.offer(custom_mime_type.into());

        self.ext_data_control_device.set_selection(Some(&source));
        self.queue.flush()?;
        Ok(source)
    }
}

impl AsFd for Connection {
    fn as_fd(&self) -> std::os::unix::prelude::BorrowedFd<'_> {
        self.conn.as_fd()
    }
}

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> std::os::unix::prelude::RawFd {
        self.conn.as_fd().as_raw_fd()
    }
}

pub(crate) fn get_objects(
    conn: &WlConnection,
    queue: &mut EventQueue<WlEventsStream>,
) -> Result<(WlSeat, ExtDataControlManagerV1, ExtDataControlDeviceV1), ConnectorError> {
    let (registry, events) = WlEventsStream::get_registry_and_registry_events_sync(conn, queue)?;

    let mut wl_seat: Option<WlSeat> = None;
    let mut ext_data_control_manager: Option<ExtDataControlManagerV1> = None;

    for event in events {
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
