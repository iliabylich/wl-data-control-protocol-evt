use std::collections::HashSet;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::ExtDataControlOfferV1;

#[derive(Default)]
pub enum OfferSeq {
    #[default]
    Empty,
    Started {
        offer: ExtDataControlOfferV1,
        mime_types: HashSet<String>,
    },
}

impl OfferSeq {
    pub(crate) fn start(&mut self, start_offer: ExtDataControlOfferV1) {
        *self = Self::Started {
            offer: start_offer,
            mime_types: HashSet::new(),
        }
    }

    pub(crate) fn extend(&mut self, mime_offer: ExtDataControlOfferV1, mime_type: String) {
        match core::mem::take(self) {
            Self::Empty => {
                log::error!("wrong sequence of events");
                log::error!("got mime type on Empty sequence");
                mime_offer.destroy();
                drop(mime_offer);
            }
            Self::Started {
                offer,
                mut mime_types,
            } => {
                if offer == mime_offer {
                    mime_types.insert(mime_type);
                    *self = Self::Started { offer, mime_types };
                } else {
                    log::error!("wrong sequence of events");
                    log::error!("got mime type for different offer");
                    offer.destroy();
                    mime_offer.destroy();
                    drop(mime_offer);
                    *self = Self::Empty;
                }
            }
        }
    }

    pub(crate) fn finish(
        &mut self,
        finish_offer: ExtDataControlOfferV1,
    ) -> Option<(ExtDataControlOfferV1, HashSet<String>)> {
        match core::mem::take(self) {
            Self::Empty => {
                log::error!("wrong sequence of events");
                log::error!("can't finish Empty sequence");
                finish_offer.destroy();
                drop(finish_offer);
                None
            }
            Self::Started { offer, mime_types } => {
                if offer == finish_offer {
                    Some((offer, mime_types))
                } else {
                    log::error!("wrong sequence of events");
                    log::error!("got finish event for different offer");
                    offer.destroy();
                    finish_offer.destroy();
                    drop(finish_offer);
                    *self = Self::Empty;
                    None
                }
            }
        }
    }

    pub(crate) fn destroy(&mut self) {
        match core::mem::take(self) {
            Self::Empty => {}
            Self::Started { offer, .. } => offer.destroy(),
        }
    }
}
