use anyhow::{Context, Result, bail};
use rustix::{
    event::{PollFd, PollFlags, poll},
    fs::Timespec,
    time::{
        Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, timerfd_create,
        timerfd_settime,
    },
};
use std::os::fd::{AsFd, OwnedFd};
use wl_data_control_protocol_evt::{ExtDataControlEvent, ExtDataControlStream};

fn main() -> Result<()> {
    env_logger::init();

    let timer = Timer::new()?;
    let mut stream = ExtDataControlStream::new()?;
    let mut tick = 0;

    loop {
        let mut pollfds = [
            PollFd::new(&timer, PollFlags::IN),
            PollFd::new(&stream, PollFlags::IN),
        ];
        poll(&mut pollfds, None).context("poll() failed")?;
        let revents = [pollfds[0].revents(), pollfds[1].revents()];

        // timer
        match REvents::new(revents[0]) {
            Some(REvents::Readable) => {
                tick += 1;
                log::trace!("tick {tick}");
                timer.read()?;

                if tick != 0 && tick % 5 == 0 {
                    let text = format!("text{tick}");
                    log::info!("pasting {text:?}");
                    stream.offer_text(text)?;
                }
            }
            Some(REvents::Err) => bail!("polling timer returned err"),
            None => {}
        }

        // ExtDataControlStream
        match REvents::new(revents[1]) {
            Some(REvents::Readable) => {
                for event in stream.drain()? {
                    log::info!("{event:?}");

                    if let ExtDataControlEvent::Received(text) = event
                        && text == "EXIT"
                    {
                        return Ok(());
                    }
                }
            }
            Some(REvents::Err) => bail!("polling ExtDataControlEvent returned err"),
            None => {}
        }
    }
}

struct Timer {
    timerfd: OwnedFd,
}

impl Timer {
    fn new() -> Result<Self> {
        let timerfd = timerfd_create(TimerfdClockId::Monotonic, TimerfdFlags::NONBLOCK)
            .context("timerfd_create() failed")?;
        timerfd_settime(
            &timerfd,
            TimerfdTimerFlags::ABSTIME,
            &Itimerspec {
                it_interval: Timespec {
                    tv_sec: 1,
                    tv_nsec: 0,
                },
                it_value: Timespec {
                    tv_sec: 1,
                    tv_nsec: 0,
                },
            },
        )
        .context("timerfd_settime() failed")?;
        Ok(Self { timerfd })
    }

    fn read(&self) -> Result<()> {
        let mut buf = [0_u8; 8];
        let bytes_read = rustix::io::read(&self.timerfd, &mut buf)?;
        assert_eq!(bytes_read, 8);
        Ok(())
    }
}

impl AsFd for Timer {
    fn as_fd(&self) -> std::os::unix::prelude::BorrowedFd<'_> {
        self.timerfd.as_fd()
    }
}

enum REvents {
    Readable,
    Err,
}

impl REvents {
    fn new(revents: PollFlags) -> Option<Self> {
        if revents.intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL) {
            Some(Self::Err)
        } else if revents.contains(PollFlags::IN) {
            Some(Self::Readable)
        } else {
            None
        }
    }
}
