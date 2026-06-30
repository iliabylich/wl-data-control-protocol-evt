use anyhow::Result;
use rustix::fd::AsFd;
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

pub(crate) fn set_nonblocking(fd: impl AsFd) -> Result<()> {
    let mut flags = fcntl_getfl(&fd)?;
    flags.insert(OFlags::NONBLOCK);
    fcntl_setfl(fd, flags)?;
    Ok(())
}
