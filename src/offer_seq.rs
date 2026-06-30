use std::collections::HashSet;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::ExtDataControlOfferV1;

pub(crate) enum OfferSeq {
    Empty,
    Started {
        offer: ExtDataControlOfferV1,
        mimes: HashSet<String>,
    },
}

pub(crate) enum FinishedOfferSeq {
    Ok(ExtDataControlOfferV1, HashSet<String>),
    Mismatch(ExtDataControlOfferV1, ExtDataControlOfferV1),
    Err(ExtDataControlOfferV1),
}

impl OfferSeq {
    pub(crate) fn start(&mut self, start_offer: ExtDataControlOfferV1) {
        self.destroy();
        *self = Self::Started {
            offer: start_offer,
            mimes: HashSet::new(),
        }
    }

    pub(crate) fn extend(&mut self, mime_offer: &ExtDataControlOfferV1, mime_type: String) {
        if let Self::Started { mimes, offer } = self {
            if offer == mime_offer {
                mimes.insert(mime_type);
            } else {
                log::error!("Wrong sequence of events");
                log::error!("Got mime type offer for a different offer");
                self.destroy();
                return;
            }
        } else {
            log::error!("Wrong sequence of events");
            log::error!("Got mime typer offer before receiing a data offer");
        }
    }

    fn take(&mut self) -> Self {
        let mut this = Self::Empty;
        core::mem::swap(self, &mut this);
        this
    }

    pub(crate) fn finish(&mut self, finish_offer: ExtDataControlOfferV1) -> FinishedOfferSeq {
        match self.take() {
            Self::Started { offer, mimes } if offer == finish_offer => {
                FinishedOfferSeq::Ok(offer, mimes)
            }
            Self::Started { offer, .. } => FinishedOfferSeq::Mismatch(offer, finish_offer),
            Self::Empty => FinishedOfferSeq::Err(finish_offer),
        }
    }

    pub(crate) fn destroy(&mut self) {
        if let Self::Started { offer, .. } = self.take() {
            offer.destroy();
        }
    }
}
