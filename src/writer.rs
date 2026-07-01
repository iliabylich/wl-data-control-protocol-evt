use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    io::Errno,
};
use std::{
    io::ErrorKind,
    os::fd::{AsFd, AsRawFd, OwnedFd},
};

pub struct Writer {
    fd: OwnedFd,
    buf: Vec<u8>,
    pos: usize,
}

pub enum WriteResult {
    Done,
    Pending,
}

fn set_nonblocking(fd: impl AsFd) -> Result<(), Errno> {
    let mut flags = fcntl_getfl(&fd)?;
    flags.insert(OFlags::NONBLOCK);
    fcntl_setfl(fd, flags)?;
    Ok(())
}

#[expect(clippy::indexing_slicing, clippy::arithmetic_side_effects)]
impl Writer {
    pub(crate) fn new(fd: OwnedFd, text: String) -> Result<Self, Errno> {
        set_nonblocking(&fd)?;

        Ok(Self {
            fd,
            buf: text.into_bytes(),
            pos: 0,
        })
    }

    pub(crate) fn write(&mut self) -> Result<WriteResult, Errno> {
        match rustix::io::write(&self.fd, &self.buf[self.pos..]) {
            Ok(bytes_written) => {
                self.pos += bytes_written;
                if self.pos == self.buf.len() {
                    Ok(WriteResult::Done)
                } else {
                    Ok(WriteResult::Pending)
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => Ok(WriteResult::Pending),
            Err(err) => Err(err),
        }
    }
}

impl AsFd for Writer {
    fn as_fd(&self) -> std::os::unix::prelude::BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsRawFd for Writer {
    fn as_raw_fd(&self) -> std::os::unix::prelude::RawFd {
        self.fd.as_raw_fd()
    }
}
