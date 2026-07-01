mod app_connection;
mod epoll;
mod ext_data_control_stream;
mod mime_types;
mod offer_seq;
mod reader;
mod rw_stream;
mod wl_event;
mod wl_events_stream;
mod writer;

use ext_data_control_stream::{ExtDataControlEvent, ExtDataControlStream};
use rustix::{
    event::{PollFd, PollFlags, poll},
    fs::Timespec,
    time::{
        Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, timerfd_create,
        timerfd_settime,
    },
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let timerfd = timerfd_create(TimerfdClockId::Monotonic, TimerfdFlags::NONBLOCK)?;
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
    )?;

    let mut state = ExtDataControlStream::new()?;
    let mut tick = 5;

    'outer: loop {
        let mut pollfds = [
            PollFd::new(&timerfd, PollFlags::IN),
            PollFd::new(&state, PollFlags::IN),
        ];
        poll(&mut pollfds, None)?;
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
            Some(REvents::Err) => panic!("timer returned err"),
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
            Some(REvents::Err) => panic!("crate returned err"),
            None => {}
        }
    }

    state.cleanup();

    Ok(())
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
