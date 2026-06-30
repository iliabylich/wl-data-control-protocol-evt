use std::os::fd::OwnedFd;
use wayland_client::{
    Proxy as _,
    protocol::{wl_registry::Event as RegistryEvent, wl_seat::WlSeat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::Event as DataControlDeviceEvent,
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{Event as DataControlOfferEvent, ExtDataControlOfferV1},
    ext_data_control_source_v1::{Event as DataControlSourceEvent, ExtDataControlSourceV1},
};

#[derive(Debug)]
pub(crate) enum WlRegistryEvent {
    WlSeat { name: u32, version: u32 },
    ExtDataControlManager { name: u32, version: u32 },
    Other,
}

impl TryFrom<RegistryEvent> for WlRegistryEvent {
    type Error = RegistryEvent;

    fn try_from(event: RegistryEvent) -> Result<Self, Self::Error> {
        match event {
            RegistryEvent::Global {
                name,
                interface,
                version,
            } => {
                if interface == WlSeat::interface().name {
                    Ok(Self::WlSeat { name, version })
                } else if interface == ExtDataControlManagerV1::interface().name {
                    Ok(Self::ExtDataControlManager { name, version })
                } else {
                    Ok(Self::Other)
                }
            }
            RegistryEvent::GlobalRemove { .. } => Ok(Self::Other),

            unsupported => Err(unsupported),
        }
    }
}

#[derive(Debug)]
pub(crate) enum WlOfferEvent {
    DataOffer(ExtDataControlOfferV1),
    Selection(Option<ExtDataControlOfferV1>),
    PrimarySelection(Option<ExtDataControlOfferV1>),
    MimeTime(ExtDataControlOfferV1, String),
    Finished,
}

#[derive(Debug)]
pub(crate) enum WlSourceEvent {
    Requested(ExtDataControlSourceV1, String, OwnedFd),
    Cancelled(ExtDataControlSourceV1),
}

#[derive(Debug)]
pub(crate) enum WlEvent {
    Offer(WlOfferEvent),
    Source(WlSourceEvent),
}

impl TryFrom<DataControlDeviceEvent> for WlEvent {
    type Error = DataControlDeviceEvent;

    fn try_from(event: DataControlDeviceEvent) -> Result<Self, Self::Error> {
        match event {
            DataControlDeviceEvent::DataOffer { id } => {
                Ok(Self::Offer(WlOfferEvent::DataOffer(id)))
            }
            DataControlDeviceEvent::Selection { id } => {
                Ok(Self::Offer(WlOfferEvent::Selection(id)))
            }
            DataControlDeviceEvent::PrimarySelection { id } => {
                Ok(Self::Offer(WlOfferEvent::PrimarySelection(id)))
            }
            DataControlDeviceEvent::Finished => Ok(Self::Offer(WlOfferEvent::Finished)),

            unsupported => Err(unsupported),
        }
    }
}

impl TryFrom<(ExtDataControlOfferV1, DataControlOfferEvent)> for WlEvent {
    type Error = DataControlOfferEvent;

    fn try_from(
        (offer, event): (ExtDataControlOfferV1, DataControlOfferEvent),
    ) -> Result<Self, Self::Error> {
        match event {
            DataControlOfferEvent::Offer { mime_type } => {
                Ok(Self::Offer(WlOfferEvent::MimeTime(offer, mime_type)))
            }

            unsupported => Err(unsupported),
        }
    }
}

impl TryFrom<(ExtDataControlSourceV1, DataControlSourceEvent)> for WlEvent {
    type Error = DataControlSourceEvent;

    fn try_from(
        (source, event): (ExtDataControlSourceV1, DataControlSourceEvent),
    ) -> Result<Self, Self::Error> {
        match event {
            DataControlSourceEvent::Send { mime_type, fd } => Ok(Self::Source(
                WlSourceEvent::Requested(source, mime_type, fd),
            )),
            DataControlSourceEvent::Cancelled => Ok(Self::Source(WlSourceEvent::Cancelled(source))),

            unsupported => Err(unsupported),
        }
    }
}
