use crate::{reader::Reader, writer::Writer};
use rustix::{buffer::spare_capacity, event::epoll, fs::Timespec, io::Errno};
use std::{
    collections::HashMap,
    os::fd::{AsFd, AsRawFd, OwnedFd},
};

pub(crate) struct Epoll {
    epollfd: OwnedFd,
    wl_fd: i32,
}

impl Epoll {
    pub(crate) fn new(wl_fd: &impl AsFd) -> Result<Self, Errno> {
        let epollfd = epoll::create(epoll::CreateFlags::CLOEXEC)?;

        epoll::add(
            &epollfd,
            wl_fd,
            epoll::EventData::new_u64(wl_fd.as_fd().as_raw_fd() as u64),
            epoll::EventFlags::IN,
        )?;

        Ok(Self {
            epollfd,
            wl_fd: wl_fd.as_fd().as_raw_fd(),
        })
    }

    pub(crate) fn add_reader(&mut self, reader: &Reader) -> Result<(), Errno> {
        epoll::add(
            &self.epollfd,
            reader,
            epoll::EventData::new_u64(reader.as_raw_fd() as u64),
            epoll::EventFlags::IN,
        )
    }

    pub(crate) fn add_writer(&mut self, writer: &Writer) -> Result<(), Errno> {
        epoll::add(
            &self.epollfd,
            writer,
            epoll::EventData::new_u64(writer.as_raw_fd() as u64),
            epoll::EventFlags::OUT,
        )
    }

    pub(crate) fn delete(&mut self, fd: &impl AsFd) -> Result<(), Errno> {
        epoll::delete(&self.epollfd, fd)
    }

    pub(crate) fn wait(
        &mut self,
        epoll_events: &mut Vec<epoll::Event>,
        timeout: Option<&Timespec>,
        readers: &HashMap<i32, Reader>,
        writers: &HashMap<i32, Writer>,
    ) -> Result<EpollResult, Errno> {
        log::trace!("epoll_wait()...");
        epoll::wait(&self.epollfd, spare_capacity(epoll_events), timeout)?;
        EpollResult::new(epoll_events, self.wl_fd, readers, writers)
    }
}

impl AsFd for Epoll {
    fn as_fd(&self) -> std::os::unix::prelude::BorrowedFd<'_> {
        self.epollfd.as_fd()
    }
}

impl AsRawFd for Epoll {
    fn as_raw_fd(&self) -> std::os::unix::prelude::RawFd {
        self.epollfd.as_raw_fd()
    }
}

#[derive(Default)]
pub(crate) struct FdSet {
    pub(crate) ready: Vec<i32>,
    pub(crate) dead: Vec<i32>,
}

#[derive(Default)]
pub(crate) struct EpollResult {
    pub(crate) wl_is_readable: bool,
    pub(crate) readers: FdSet,
    pub(crate) writers: FdSet,
}

impl EpollResult {
    fn new(
        events: &[epoll::Event],
        wl_fd: i32,
        readers: &HashMap<i32, Reader>,
        writers: &HashMap<i32, Writer>,
    ) -> Result<Self, Errno> {
        let mut wl_is_readable = false;
        let mut readers_fd_set = FdSet::default();
        let mut writers_fd_set = FdSet::default();

        for event in events {
            let fd = event.data.u64() as i32;
            let revents: epoll::EventFlags = event.flags;

            if fd == wl_fd {
                if revents.intersects(epoll::EventFlags::HUP | epoll::EventFlags::ERR) {
                    return Err(Errno::CONNRESET);
                } else if revents.contains(epoll::EventFlags::IN) {
                    wl_is_readable = true;
                }
            } else if readers.contains_key(&fd) {
                if revents.intersects(epoll::EventFlags::ERR) {
                    log::error!("Reader with FD {fd} returned revents {revents:?}, removing it");
                    readers_fd_set.dead.push(fd);
                } else if revents.intersects(epoll::EventFlags::IN | epoll::EventFlags::HUP) {
                    readers_fd_set.ready.push(fd);
                }
            } else if writers.contains_key(&fd) {
                if revents.intersects(epoll::EventFlags::ERR | epoll::EventFlags::HUP) {
                    log::error!("Writer with FD {fd} returned revents {revents:?}, removing it");
                    writers_fd_set.dead.push(fd);
                } else if revents.contains(epoll::EventFlags::OUT) {
                    writers_fd_set.ready.push(fd);
                }
            }
        }

        Ok(EpollResult {
            wl_is_readable,
            readers: readers_fd_set,
            writers: writers_fd_set,
        })
    }
}
