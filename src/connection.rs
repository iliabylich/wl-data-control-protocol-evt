use crate::{
    wl_event::{WlEvent, WlRegistryEvent},
    wl_events_stream::WlEventsStream,
};
use crossbeam_queue::SegQueue;
use std::os::fd::{AsFd, AsRawFd};
use wayland_client::{
    ConnectError, Connection as WlConnection, DispatchError, EventQueue, backend::WaylandError,
    protocol::wl_seat::WlSeat,
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::ExtDataControlDeviceV1,
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_source_v1::ExtDataControlSourceV1,
};

static WL_REGISTRY_EVENTS_QUEUE: SegQueue<WlRegistryEvent> = SegQueue::new();
static WL_EVENTS_QUEUE: SegQueue<WlEvent> = SegQueue::new();

pub(crate) struct AppConnection {
    pub(crate) conn: WlConnection,
    pub(crate) queue: EventQueue<WlEventsStream>,

    pub(crate) wl_seat: WlSeat,
    pub(crate) ext_data_control_manager: ExtDataControlManagerV1,
    pub(crate) ext_data_control_device: ExtDataControlDeviceV1,
}

impl AppConnection {
    pub(crate) fn connect() -> Result<Self, ConnectorError> {
        let conn = WlConnection::connect_to_env()?;
        let mut queue = conn.new_event_queue::<WlEventsStream>();

        let (wl_seat, ext_data_control_manager, ext_data_control_device) =
            get_startup_objects(&conn, &mut queue)?;

        Ok(Self {
            conn,
            queue,

            wl_seat,
            ext_data_control_manager,
            ext_data_control_device,
        })
    }

    pub(crate) fn read_until_blocked(&mut self) -> Result<Vec<WlEvent>, ReadError> {
        let wl_read_guard = self
            .queue
            .prepare_read()
            .ok_or(ReadError::FailedToCreateReadGuard)?;
        wl_read_guard.read()?;
        self.queue.dispatch_pending(&mut WlEventsStream)?;

        let mut events = vec![];
        while let Some(event) = WL_EVENTS_QUEUE.pop() {
            events.push(event);
        }

        Ok(events)
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
            .create_data_source(&self.queue.handle(), &WL_EVENTS_QUEUE);
        source.offer("text/plain;charset=utf-8".to_string());
        source.offer("text/plain".to_string());
        source.offer(custom_mime_type.into());

        self.ext_data_control_device.set_selection(Some(&source));
        self.queue.flush()?;
        Ok(source)
    }

    pub(crate) fn wl_events_queue() -> &'static SegQueue<WlEvent> {
        &WL_EVENTS_QUEUE
    }
}

impl AsFd for AppConnection {
    fn as_fd(&self) -> std::os::unix::prelude::BorrowedFd<'_> {
        self.conn.as_fd()
    }
}

impl AsRawFd for AppConnection {
    fn as_raw_fd(&self) -> std::os::unix::prelude::RawFd {
        self.conn.as_fd().as_raw_fd()
    }
}

pub(crate) fn get_startup_objects(
    conn: &WlConnection,
    queue: &mut EventQueue<WlEventsStream>,
) -> Result<(WlSeat, ExtDataControlManagerV1, ExtDataControlDeviceV1), ConnectorError> {
    let registry = conn
        .display()
        .get_registry(&queue.handle(), &WL_REGISTRY_EVENTS_QUEUE);
    queue.roundtrip(&mut WlEventsStream)?;

    let mut wl_seat: Option<WlSeat> = None;
    let mut ext_data_control_manager: Option<ExtDataControlManagerV1> = None;

    while let Some(event) = WL_REGISTRY_EVENTS_QUEUE.pop() {
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
        ext_data_control_manager.get_data_device(&wl_seat, &queue.handle(), &WL_EVENTS_QUEUE);

    Ok((wl_seat, ext_data_control_manager, ext_data_control_device))
}

#[derive(Debug)]
pub(crate) enum ConnectorError {
    WaylandConnectFailed(ConnectError),
    WaylandDispatchFailed(DispatchError),
    NoSeat,
    Unsupported,
}

impl From<ConnectError> for ConnectorError {
    fn from(err: ConnectError) -> Self {
        Self::WaylandConnectFailed(err)
    }
}

impl From<DispatchError> for ConnectorError {
    fn from(err: DispatchError) -> Self {
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

//

#[derive(Debug)]
pub(crate) enum ReadError {
    Wayland(WaylandError),
    Dispatch(DispatchError),
    FailedToCreateReadGuard,
}

impl core::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wayland(err) => write!(f, "Wayland({err})"),
            Self::Dispatch(err) => write!(f, "Dispatch({err})"),
            Self::FailedToCreateReadGuard => write!(f, "FailedToCreateReadGuard"),
        }
    }
}

impl core::error::Error for ReadError {}

impl From<WaylandError> for ReadError {
    fn from(err: WaylandError) -> Self {
        Self::Wayland(err)
    }
}

impl From<DispatchError> for ReadError {
    fn from(err: DispatchError) -> Self {
        Self::Dispatch(err)
    }
}
