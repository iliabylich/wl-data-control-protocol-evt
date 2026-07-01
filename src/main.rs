use std::{
    collections::HashMap,
    os::fd::{AsFd, AsRawFd},
};

mod reader;
use reader::{ReadResult, Reader};

mod writer;
use writer::{WriteResult, Writer};

mod offer_seq;

mod wl_event;

mod wl_events_stream;
use wl_events_stream::WlEventsStream;

mod mime_types;

mod epoll;
use epoll::{Epoll, EpollError, EpollResult};

mod app_connection;
use app_connection::AppConnection;

mod rw_stream;
use rw_stream::{ReaderWriterEvent, ReaderWriterStream};

mod evented;

struct State {
    connection: AppConnection,
    rw_stream: ReaderWriterStream,

    epoll: Epoll,
    epoll_events: Vec<rustix::event::epoll::Event>,
    running: bool,

    readers: HashMap<i32, Reader>,
    writers: HashMap<i32, Writer>,
}

impl State {
    fn connect() -> Result<Self, Box<dyn std::error::Error>> {
        let connection = AppConnection::connect()?;
        let epoll = Epoll::new(connection.as_fd())?;

        Ok(State {
            connection,
            rw_stream: ReaderWriterStream::new(),

            epoll,
            epoll_events: Vec::with_capacity(16),
            running: true,

            readers: HashMap::new(),
            writers: HashMap::new(),
        })
    }

    fn cleanup(&mut self) {
        for reader in self.readers.values() {
            reader.destroy();
        }
        self.rw_stream.cleanup();
        self.connection.cleanup_and_flush();
    }

    fn remove_reader(&mut self, fd: i32) -> Result<(), EpollError> {
        let Some(reader) = self.readers.remove(&fd) else {
            return Ok(());
        };
        self.epoll.delete(reader.as_fd())?;
        reader.destroy();
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
        self.rw_stream.offer_text(text, &mut self.connection)?;
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
        for event in self.rw_stream.read_until_blocked(&mut self.connection)? {
            match event {
                ReaderWriterEvent::NewReader(reader) => {
                    log::trace!("new reader {:?}", reader.as_raw_fd());
                    self.epoll.register_readable(reader.as_fd())?;
                    self.readers.insert(reader.as_raw_fd(), reader);
                }
                ReaderWriterEvent::NewWriter(writer) => {
                    log::trace!("new writer {:?}", writer.as_raw_fd());
                    self.epoll.register_writable(writer.as_fd())?;
                    self.writers.insert(writer.as_raw_fd(), writer);
                }
                ReaderWriterEvent::Finished => {
                    self.running = false;
                }
            }
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
