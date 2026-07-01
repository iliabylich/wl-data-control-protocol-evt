use anyhow::{Context, Result, bail};
use rustix::{
    event::{PollFd, PollFlags, poll},
    fs::Timespec,
    time::{
        Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, timerfd_create,
        timerfd_settime,
    },
};
use std::os::fd::OwnedFd;
use wl_data_control_protocol_evt::{ExtDataControlEvent, ExtDataControlStream};

fn main() -> Result<()> {
    env_logger::init();

    let timerfd = create_timer()?;
    let mut state = ExtDataControlStream::new().context("failed to create ExtDataControlStream")?;
    let mut tick = 5;

    'outer: loop {
        let mut pollfds = [
            PollFd::new(&timerfd, PollFlags::IN),
            PollFd::new(&state, PollFlags::IN),
        ];
        poll(&mut pollfds, None).context("poll() failed")?;
        let revents = [pollfds[0].revents(), pollfds[1].revents()];

        //
        match REvents::new(revents[0]) {
            Some(REvents::Readable) => {
                log::trace!("tick {tick}");
                let mut buf = [0_u8; 8];
                let bytes_read = rustix::io::read(&timerfd, &mut buf)?;
                assert_eq!(bytes_read, 8);

                tick += 1;
                if tick % 10 == 0 {
                    state.offer_text(format!("text{tick}"))?;
                }
            }
            Some(REvents::Err) => bail!("timer returned err"),
            None => {}
        }

        //
        match REvents::new(revents[1]) {
            Some(REvents::Readable) => {
                log::trace!("wl is readable");
                for event in state.read()? {
                    println!("{event:?}");

                    if let ExtDataControlEvent::Received(text) = event
                        && text == "EXIT"
                    {
                        break 'outer;
                    }
                }
            }
            Some(REvents::Err) => bail!("crate returned err"),
            None => {}
        }
    }

    state.cleanup();

    Ok(())
}

fn create_timer() -> Result<OwnedFd> {
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
    Ok(timerfd)
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
