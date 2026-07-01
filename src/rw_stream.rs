use crate::{
    ExtDataControlReadError,
    app_connection::AppConnection,
    mime_types::MimeTypes,
    offer_seq::OfferSeq,
    reader::Reader,
    wl_event::{WlEvent, WlOfferEvent, WlSourceEvent},
    writer::Writer,
};
use rustix::pipe::PipeFlags;
use std::{collections::HashMap, os::fd::AsFd as _};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1::ExtDataControlSourceV1;

pub struct ReaderWriterStream {
    offer_seq: OfferSeq,
    source_to_text: HashMap<ExtDataControlSourceV1, String>,
    mime_types: MimeTypes,
}

impl ReaderWriterStream {
    pub(crate) fn new() -> Self {
        Self {
            offer_seq: OfferSeq::Empty,
            source_to_text: HashMap::new(),
            mime_types: MimeTypes::new(),
        }
    }

    pub(crate) fn read_until_blocked(
        &mut self,
        conn: &mut AppConnection,
    ) -> Result<Vec<ReaderWriterEvent>, ExtDataControlReadError> {
        let mut events = vec![];
        for event in conn.read_until_blocked()? {
            if let Some(event) = self.map_any(event) {
                events.push(event);
            }
        }
        Ok(events)
    }

    pub(crate) fn save_offer(&mut self, text: impl Into<String>, source: ExtDataControlSourceV1) {
        self.source_to_text.insert(source, text.into());
    }

    pub(crate) fn cleanup(&mut self) {
        self.offer_seq.destroy();
        for source in self.source_to_text.keys() {
            source.destroy();
        }
    }

    pub(crate) fn mime_type_mask(&self) -> &str {
        self.mime_types.mask()
    }

    fn map_any(&mut self, event: WlEvent) -> Option<ReaderWriterEvent> {
        match event {
            WlEvent::Offer(event) => self.map_offer(event),
            WlEvent::Source(event) => self.map_source(event),
        }
    }

    fn map_offer(&mut self, event: WlOfferEvent) -> Option<ReaderWriterEvent> {
        match event {
            WlOfferEvent::DataOffer(offer) => {
                self.offer_seq.start(offer);
            }

            WlOfferEvent::MimeTime(offer, mime_type) => {
                self.offer_seq.extend(offer, mime_type);
            }

            WlOfferEvent::Selection(Some(offer)) => {
                let (offer, mime_types) = self.offer_seq.finish(offer)?;
                let Some(mime_type_to_ask_for) = self.mime_types.choose(&mime_types) else {
                    offer.destroy();
                    return None;
                };

                match rustix::pipe::pipe_with(PipeFlags::NONBLOCK) {
                    Ok((reader, writer)) => {
                        offer.receive(mime_type_to_ask_for, writer.as_fd());
                        drop(writer);
                        return Some(ReaderWriterEvent::NewReader(Box::new(Reader::new(
                            reader, offer,
                        ))));
                    }
                    Err(err) => {
                        log::error!("failed to create pipe: {err:?}");
                        offer.destroy();
                    }
                }
            }
            WlOfferEvent::Selection(None) => {
                self.offer_seq.destroy();
            }

            WlOfferEvent::PrimarySelection(offer) => {
                self.offer_seq.destroy();
                if let Some(offer) = offer {
                    offer.destroy();
                }
            }

            WlOfferEvent::Finished => {
                log::warn!("ExtDataControlDeviceV1 has finished");
                return Some(ReaderWriterEvent::Finished);
            }
        }

        None
    }

    fn map_source(&mut self, event: WlSourceEvent) -> Option<ReaderWriterEvent> {
        match event {
            WlSourceEvent::Requested(source, mime_type, fd) => {
                if !MimeTypes::is_text(&mime_type) {
                    return None;
                }
                let text = self.source_to_text.get(&source)?;

                match Writer::new(fd, text.clone()) {
                    Ok(writer) => Some(ReaderWriterEvent::NewWriter(writer)),
                    Err(err) => {
                        log::error!("filed to create a writer: {err:?}");
                        None
                    }
                }
            }
            WlSourceEvent::Cancelled(source) => {
                self.source_to_text.remove(&source);
                source.destroy();
                None
            }
        }
    }
}

pub enum ReaderWriterEvent {
    NewReader(Box<Reader>),
    NewWriter(Writer),
    Finished,
}
