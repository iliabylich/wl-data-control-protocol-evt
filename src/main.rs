use rustix::pipe::PipeFlags;
use std::{
    collections::HashMap,
    os::fd::{AsFd, AsRawFd},
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1::ExtDataControlSourceV1;

mod reader;
use reader::{ReadResult, Reader};

mod writer;
use writer::{WriteResult, Writer};

mod offer_seq;
use offer_seq::OfferSeq;

mod wl_event;
use wl_event::{WlEvent, WlOfferEvent, WlSourceEvent};

mod wl_events_stream;
use wl_events_stream::WlEventsStream;

mod mime_types;
use mime_types::MimeTypes;

mod epoll;
use epoll::{Epoll, EpollError, EpollResult};

mod connection;
use connection::AppConnection;

mod evented;

struct State {
    connection: AppConnection,
    epoll: Epoll,
    epoll_events: Vec<rustix::event::epoll::Event>,
    running: bool,

    readers: HashMap<i32, Reader>,
    writers: HashMap<i32, Writer>,
    mime_types: MimeTypes,

    offer_seq: OfferSeq,

    source_to_text: HashMap<ExtDataControlSourceV1, String>,
}

impl State {
    fn connect() -> Result<Self, Box<dyn std::error::Error>> {
        let connection = AppConnection::connect()?;

        Ok(State {
            running: true,
            epoll: Epoll::new(connection.as_fd())?,
            epoll_events: Vec::with_capacity(16),

            connection,
            readers: HashMap::new(),
            writers: HashMap::new(),
            mime_types: MimeTypes::new(),

            offer_seq: OfferSeq::Empty,
            source_to_text: HashMap::new(),
        })
    }

    fn handle(&mut self, event: WlEvent) -> Result<(), EpollError> {
        match event {
            WlEvent::Offer(event) => self.handle_offer_event(event)?,
            WlEvent::Source(event) => self.handle_source_event(event)?,
        }

        Ok(())
    }

    fn handle_offer_event(&mut self, event: WlOfferEvent) -> Result<(), EpollError> {
        match event {
            WlOfferEvent::DataOffer(offer) => {
                self.offer_seq.start(offer);
            }

            WlOfferEvent::MimeTime(offer, mime_type) => {
                self.offer_seq.extend(offer, mime_type);
            }

            WlOfferEvent::Selection(Some(offer)) => {
                let Some((offer, mime_types)) = self.offer_seq.finish(offer) else {
                    return Ok(());
                };
                let Some(mime_type_to_ask_for) = self.mime_types.choose(mime_types) else {
                    offer.destroy();
                    return Ok(());
                };

                match rustix::pipe::pipe_with(PipeFlags::NONBLOCK) {
                    Ok((reader, writer)) => {
                        offer.receive(mime_type_to_ask_for, writer.as_fd());
                        drop(writer);
                        self.add_reader(Reader::new(reader, offer))?;
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
                self.running = false;
            }
        }

        Ok(())
    }

    fn handle_source_event(&mut self, event: WlSourceEvent) -> Result<(), EpollError> {
        match event {
            WlSourceEvent::Requested(source, mime_type, fd) => {
                if !MimeTypes::is_text(&mime_type) {
                    return Ok(());
                }
                let Some(text) = self.source_to_text.get(&source) else {
                    return Ok(());
                };

                match Writer::new(fd, text.clone()) {
                    Ok(writer) => self.add_writer(writer)?,
                    Err(err) => log::error!("{err:?}"),
                }
            }
            WlSourceEvent::Cancelled(source) => {
                self.source_to_text.remove(&source);
                source.destroy();
            }
        }
        Ok(())
    }

    fn cleanup(&mut self) {
        for reader in self.readers.values() {
            reader.destroy();
        }
        self.offer_seq.destroy();
        for source in self.source_to_text.keys() {
            source.destroy()
        }

        self.connection.cleanup_and_flush();
    }

    fn add_reader(&mut self, reader: Reader) -> Result<(), EpollError> {
        log::trace!("new reader {:?}", reader.as_raw_fd());
        self.epoll.add_reader(&reader)?;
        self.readers.insert(reader.as_raw_fd(), reader);
        Ok(())
    }

    fn remove_reader(&mut self, fd: i32) -> Result<(), EpollError> {
        let Some(reader) = self.readers.remove(&fd) else {
            return Ok(());
        };
        self.epoll.delete(reader.as_fd())?;
        reader.destroy();
        Ok(())
    }

    fn add_writer(&mut self, writer: Writer) -> Result<(), EpollError> {
        log::trace!("new writer {:?}", writer.as_raw_fd());
        self.epoll.add_writer(&writer)?;
        self.writers.insert(writer.as_raw_fd(), writer);
        Ok(())
    }

    fn remove_writer(&mut self, fd: i32) -> Result<(), EpollError> {
        let Some(writer) = self.writers.remove(&fd) else {
            return Ok(());
        };
        self.epoll.delete(writer.as_fd())?;
        Ok(())
    }

    fn offer_text(&mut self, text: String) -> Result<(), wayland_client::backend::WaylandError> {
        let source = self.connection.offer_text(self.mime_types.mask())?;
        self.source_to_text.insert(source, text);
        Ok(())
    }

    fn handle_epoll_result(
        &mut self,
        epoll_result: EpollResult,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let EpollResult {
            wl_is_readable,
            readers,
            writers,
        } = epoll_result;

        if wl_is_readable {
            self.handle_wl_socket()?;
        }

        for fd in readers.dead {
            self.remove_reader(fd)?;
        }

        for fd in writers.dead {
            self.remove_writer(fd)?;
        }

        for fd in readers.ready {
            self.handle_reader(fd)?;
        }

        for fd in writers.ready {
            self.handle_writer(fd)?;
        }

        Ok(())
    }

    fn handle_wl_socket(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        for event in self.connection.read_until_blocked()? {
            self.handle(event)?;
        }

        Ok(())
    }

    fn handle_reader(&mut self, fd: i32) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(reader) = self.readers.get_mut(&fd) {
            log::trace!("Reading {fd:?}");
            match reader.read() {
                Ok(ReadResult::Done(text)) => {
                    log::trace!("Got text {text:?}");
                    self.remove_reader(fd)?;

                    if text == "EXIT" {
                        self.running = false;
                    }
                }
                Ok(ReadResult::Pending) => {}
                Err(err) => {
                    log::error!("reader {fd:?} returned error {err:?}");
                    self.remove_reader(fd)?;
                }
            }
        }

        Ok(())
    }

    fn handle_writer(&mut self, fd: i32) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(writer) = self.writers.get_mut(&fd) {
            log::trace!("Writing {fd:?}");
            match writer.write() {
                Ok(WriteResult::Done) => {
                    log::trace!("Done writing to {fd:?}");
                    self.remove_writer(fd)?;
                }
                Ok(WriteResult::Pending) => {}
                Err(err) => {
                    log::error!("writer {fd:?} returned error {err:?}");
                    self.remove_writer(fd)?;
                }
            }
        }
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut state = State::connect()?;
    state.offer_text(String::from("BOO"))?;

    while state.running {
        let epoll_result = state.epoll.wait(
            &mut state.epoll_events,
            None,
            state.connection.as_fd(),
            &state.readers,
            &state.writers,
        )?;
        state.epoll_events.clear();

        state.handle_epoll_result(epoll_result)?;

        state.connection.queue.flush()?;
        state
            .connection
            .queue
            .dispatch_pending(&mut WlEventsStream)?;
    }

    state.cleanup();

    Ok(())
}
