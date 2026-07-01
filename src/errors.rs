use crate::EpollError;
use wayland_client::{ConnectError, DispatchError, backend::WaylandError};

#[derive(Debug)]
pub enum ExtDataControlConnectError {
    ConnectError(ConnectError),
    DispatchError(DispatchError),
    WaylandError(WaylandError),
    EpollError(EpollError),
    NoSeat,
    Unsupported,
}

impl core::fmt::Display for ExtDataControlConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ConnectError(err) => write!(f, "ConnectError({err})"),
            Self::DispatchError(err) => write!(f, "DispatchError({err})"),
            Self::WaylandError(err) => write!(f, "WaylandError({err})"),
            Self::EpollError(err) => write!(f, "EpollError({err})"),
            Self::NoSeat => write!(f, "NoSeat"),
            Self::Unsupported => write!(f, "Unsupported"),
        }
    }
}

impl core::error::Error for ExtDataControlConnectError {}

impl From<ConnectError> for ExtDataControlConnectError {
    fn from(err: ConnectError) -> Self {
        Self::ConnectError(err)
    }
}

impl From<DispatchError> for ExtDataControlConnectError {
    fn from(err: DispatchError) -> Self {
        Self::DispatchError(err)
    }
}

impl From<WaylandError> for ExtDataControlConnectError {
    fn from(err: WaylandError) -> Self {
        Self::WaylandError(err)
    }
}

impl From<EpollError> for ExtDataControlConnectError {
    fn from(err: EpollError) -> Self {
        Self::EpollError(err)
    }
}

//

#[derive(Debug)]
pub enum ExtDataControlReadError {
    WaylandError(WaylandError),
    DispatchError(DispatchError),
    FailedToCreateReadGuard,
    EpollError(EpollError),
}

impl core::fmt::Display for ExtDataControlReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WaylandError(err) => write!(f, "WaylandError({err})"),
            Self::DispatchError(err) => write!(f, "DispatchError({err})"),
            Self::FailedToCreateReadGuard => write!(f, "FailedToCreateReadGuard"),
            Self::EpollError(err) => write!(f, "EpollError({err})"),
        }
    }
}

impl core::error::Error for ExtDataControlReadError {}

impl From<EpollError> for ExtDataControlReadError {
    fn from(err: EpollError) -> Self {
        Self::EpollError(err)
    }
}

impl From<WaylandError> for ExtDataControlReadError {
    fn from(err: WaylandError) -> Self {
        Self::WaylandError(err)
    }
}

impl From<DispatchError> for ExtDataControlReadError {
    fn from(err: DispatchError) -> Self {
        Self::DispatchError(err)
    }
}
