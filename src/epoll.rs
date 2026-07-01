use rustix::{
    buffer::spare_capacity,
    event::epoll::{self, EventFlags},
    fs::Timespec,
    io::Errno,
};
use std::{
    collections::HashMap,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
};

pub(crate) struct Epoll {
    epollfd: OwnedFd,
}

#[derive(Debug)]
pub struct EpollError(Errno);

impl From<Errno> for EpollError {
    fn from(errno: Errno) -> Self {
        Self(errno)
    }
}

impl core::fmt::Display for EpollError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "EpollError({})", self.0)
    }
}

impl core::error::Error for EpollError {}

impl Epoll {
    pub(crate) fn new(wl_fd: BorrowedFd<'_>) -> Result<Self, EpollError> {
        let epollfd = epoll::create(epoll::CreateFlags::CLOEXEC)?;
        let mut this = Self { epollfd };
        this.register(wl_fd, EventFlags::IN)?;
        Ok(this)
    }

    fn register(&mut self, fd: BorrowedFd<'_>, flags: EventFlags) -> Result<(), EpollError> {
        epoll::add(
            &self.epollfd,
            fd,
            epoll::EventData::new_u64(fd.as_fd().as_raw_fd() as u64),
            flags,
        )?;
        Ok(())
    }

    pub(crate) fn register_readable(&mut self, fd: BorrowedFd<'_>) -> Result<(), EpollError> {
        self.register(fd, EventFlags::IN)
    }

    pub(crate) fn register_writable(&mut self, fd: BorrowedFd<'_>) -> Result<(), EpollError> {
        self.register(fd, EventFlags::OUT)
    }

    pub(crate) fn delete(&mut self, fd: BorrowedFd<'_>) -> Result<(), EpollError> {
        epoll::delete(&self.epollfd, fd)?;
        Ok(())
    }

    pub(crate) fn wait<R, W>(
        &mut self,
        epoll_events: &mut Vec<epoll::Event>,
        timeout: Option<&Timespec>,
        wl_fd: BorrowedFd<'_>,
        readers: &HashMap<i32, R>,
        writers: &HashMap<i32, W>,
    ) -> Result<EpollResult, EpollError> {
        epoll::wait(&self.epollfd, spare_capacity(epoll_events), timeout)?;
        EpollResult::new(epoll_events, wl_fd, readers, writers)
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
    fn new<R, W>(
        events: &[epoll::Event],
        wl_fd: BorrowedFd<'_>,
        readers: &HashMap<i32, R>,
        writers: &HashMap<i32, W>,
    ) -> Result<Self, EpollError> {
        let mut wl_is_readable = false;
        let mut readers_fd_set = FdSet::default();
        let mut writers_fd_set = FdSet::default();

        for event in events {
            let fd = event.data.u64() as i32;
            let revents: epoll::EventFlags = event.flags;

            if fd == wl_fd.as_raw_fd() {
                if revents.intersects(epoll::EventFlags::HUP | epoll::EventFlags::ERR) {
                    return Err(EpollError(Errno::CONNRESET));
                } else if revents.contains(epoll::EventFlags::IN) {
                    wl_is_readable = true;
                }
            } else if readers.contains_key(&fd) {
                if revents.intersects(epoll::EventFlags::ERR) {
                    log::error!("reader with FD {fd} returned revents {revents:?}, removing it");
                    readers_fd_set.dead.push(fd);
                } else if revents.intersects(epoll::EventFlags::IN | epoll::EventFlags::HUP) {
                    readers_fd_set.ready.push(fd);
                }
            } else if writers.contains_key(&fd) {
                if revents.intersects(epoll::EventFlags::ERR | epoll::EventFlags::HUP) {
                    log::error!("writer with FD {fd} returned revents {revents:?}, removing it");
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
