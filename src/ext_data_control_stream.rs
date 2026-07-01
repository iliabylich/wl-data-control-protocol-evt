use crate::{
    ExtDataControlConnectError, ExtDataControlReadError,
    app_connection::AppConnection,
    epoll::{Epoll, EpollError, EpollResult},
    reader::{ReadResult, Reader},
    rw_stream::{ReaderWriterEvent, ReaderWriterStream},
    wl_events_stream::WlEventsStream,
    writer::{WriteResult, Writer},
};
use rustix::fs::Timespec;
use std::{
    collections::HashMap,
    os::fd::{AsFd, AsRawFd},
};
use wayland_client::backend::WaylandError;

pub struct ExtDataControlStream {
    connection: AppConnection,
    rw_stream: ReaderWriterStream,

    epoll: Epoll,
    epoll_events: Vec<rustix::event::epoll::Event>,

    readers: HashMap<i32, Reader>,
    writers: HashMap<i32, Writer>,
}

impl ExtDataControlStream {
    pub fn new() -> Result<Self, ExtDataControlConnectError> {
        let connection = AppConnection::connect()?;
        let epoll = Epoll::new(connection.as_fd())?;

        Ok(Self {
            connection,
            rw_stream: ReaderWriterStream::new(),

            epoll,
            epoll_events: Vec::with_capacity(16),

            readers: HashMap::new(),
            writers: HashMap::new(),
        })
    }

    pub fn cleanup(&mut self) {
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

    pub fn offer_text(&mut self, text: impl Into<String>) -> Result<(), WaylandError> {
        let source = self
            .connection
            .offer_text(self.rw_stream.mime_type_mask())?;
        self.rw_stream.save_offer(text, source)?;
        Ok(())
    }

    fn process_epoll_result(
        &mut self,
        epoll_result: EpollResult,
        events: &mut Vec<ExtDataControlEvent>,
    ) -> Result<(), ExtDataControlReadError> {
        let EpollResult {
            wl_is_readable,
            readers,
            writers,
        } = epoll_result;

        if wl_is_readable {
            self.read_from_wl_socket_until_blocked(events)?;
        }

        for fd in readers.dead {
            self.remove_reader(fd)?;
        }

        for fd in writers.dead {
            self.remove_writer(fd)?;
        }

        for fd in readers.ready {
            self.read_offer(fd, events)?;
        }

        for fd in writers.ready {
            self.write_source(fd)?;
        }

        Ok(())
    }

    fn read_from_wl_socket_until_blocked(
        &mut self,
        events: &mut Vec<ExtDataControlEvent>,
    ) -> Result<(), ExtDataControlReadError> {
        for event in self.rw_stream.read_until_blocked(&mut self.connection)? {
            match event {
                ReaderWriterEvent::NewReader(reader) => {
                    self.epoll.register_readable(reader.as_fd())?;
                    self.readers.insert(reader.as_raw_fd(), *reader);
                }
                ReaderWriterEvent::NewWriter(writer) => {
                    self.epoll.register_writable(writer.as_fd())?;
                    self.writers.insert(writer.as_raw_fd(), writer);
                }
                ReaderWriterEvent::Finished => events.push(ExtDataControlEvent::Finished),
            }
        }

        Ok(())
    }

    fn read_offer(
        &mut self,
        fd: i32,
        events: &mut Vec<ExtDataControlEvent>,
    ) -> Result<(), ExtDataControlReadError> {
        if let Some(reader) = self.readers.get_mut(&fd) {
            match reader.read() {
                Ok(ReadResult::Done(text)) => {
                    self.remove_reader(fd)?;
                    events.push(ExtDataControlEvent::Received(text));
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

    fn write_source(&mut self, fd: i32) -> Result<(), ExtDataControlReadError> {
        if let Some(writer) = self.writers.get_mut(&fd) {
            match writer.write() {
                Ok(WriteResult::Done) => {
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

    pub fn read(&mut self) -> Result<Vec<ExtDataControlEvent>, ExtDataControlReadError> {
        let epoll_result = self.epoll.wait(
            &mut self.epoll_events,
            Some(&Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }),
            self.connection.as_fd(),
            &self.readers,
            &self.writers,
        )?;
        self.epoll_events.clear();

        let mut events = vec![];
        self.process_epoll_result(epoll_result, &mut events)?;

        self.connection.queue.flush()?;
        self.connection
            .queue
            .dispatch_pending(&mut WlEventsStream)?;

        Ok(events)
    }
}

impl AsFd for ExtDataControlStream {
    fn as_fd(&self) -> std::os::unix::prelude::BorrowedFd<'_> {
        self.epoll.as_fd()
    }
}

impl AsRawFd for ExtDataControlStream {
    fn as_raw_fd(&self) -> std::os::unix::prelude::RawFd {
        self.epoll.as_raw_fd()
    }
}

#[derive(Debug)]
pub enum ExtDataControlEvent {
    Received(String),
    Finished,
}
