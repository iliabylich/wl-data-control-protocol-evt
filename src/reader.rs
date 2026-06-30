use rustix::io::Errno;
use std::{
    io::ErrorKind,
    os::fd::{AsFd, AsRawFd, OwnedFd},
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::ExtDataControlOfferV1;

pub(crate) struct Reader {
    fd: OwnedFd,
    buf: [u8; 1_024],
    len: usize,
    offer: ExtDataControlOfferV1,
}

pub(crate) enum ReadResult {
    Done(String),
    Pending,
}

#[derive(Debug)]
pub(crate) enum ReadError {
    Errno(Errno),
    GotNonUtf8,
}

impl core::fmt::Display for ReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ReadError::Errno(errno) => write!(f, "{errno}"),
            ReadError::GotNonUtf8 => write!(f, "GotNonUtf8"),
        }
    }
}

impl core::error::Error for ReadError {}

impl Reader {
    pub(crate) fn new(fd: OwnedFd, offer: ExtDataControlOfferV1) -> Self {
        Self {
            fd,
            buf: [0; _],
            len: 0,
            offer,
        }
    }

    pub(crate) fn read(&mut self) -> Result<ReadResult, ReadError> {
        match rustix::io::read(&self.fd, &mut self.buf[self.len..]) {
            Ok(bytes_read) => {
                self.len += bytes_read;
                if bytes_read == 0 || self.len == self.buf.len() {
                    self.as_string()
                } else {
                    Ok(ReadResult::Pending)
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => Ok(ReadResult::Pending),
            Err(err) => Err(ReadError::Errno(err)),
        }
    }

    fn as_string(&self) -> Result<ReadResult, ReadError> {
        match core::str::from_utf8(&self.buf[..self.len]) {
            Ok(s) => Ok(ReadResult::Done(s.to_string())),
            Err(err) if self.len == self.buf.len() => {
                let valid = err.valid_up_to();
                Ok(ReadResult::Done(
                    String::from_utf8_lossy(&self.buf[..valid]).into_owned(),
                ))
            }
            Err(_) => Err(ReadError::GotNonUtf8),
        }
    }

    pub(crate) fn destroy(&self) {
        self.offer.destroy();
    }
}

impl AsFd for Reader {
    fn as_fd(&self) -> std::os::unix::prelude::BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsRawFd for Reader {
    fn as_raw_fd(&self) -> std::os::unix::prelude::RawFd {
        self.fd.as_raw_fd()
    }
}
