use anyhow::Result;
use std::{os::fd::AsRawFd, time::Duration};
use tokio::io::unix::AsyncFd;
use wl_data_control_protocol_evt::{ExtDataControlEvent, ExtDataControlStream};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let mut stream = AsyncExtDataControlStream::new()?;
    let mut timer = tokio::time::interval(Duration::from_secs(1));
    let mut tick = 0;

    loop {
        tokio::select! {
            _ = timer.tick() => {
                tick += 1;
                log::trace!("tick {tick}");
                if tick != 0 && tick % 5 == 0 {
                    let text = format!("text{tick}");
                    log::info!("pasting {text:?}");
                    stream.offer_text(text)?;
                }
            }

            events = stream.drain() => {
                for event in events? {
                    log::info!("{event:?}");

                    if let ExtDataControlEvent::Received(text) = event
                        && text == "EXIT"
                    {
                        return Ok(());
                    }
                }
            }
        }
    }
}

struct AsyncExtDataControlStream {
    inner: ExtDataControlStream,
    async_fd: AsyncFd<i32>,
}

impl AsyncExtDataControlStream {
    fn new() -> Result<Self> {
        let stream = ExtDataControlStream::new()?;
        let async_fd = AsyncFd::new(stream.as_raw_fd())?;
        Ok(Self {
            inner: stream,
            async_fd,
        })
    }

    fn offer_text(&mut self, text: String) -> Result<()> {
        self.inner.offer_text(text)?;
        Ok(())
    }

    async fn drain(&mut self) -> Result<Vec<ExtDataControlEvent>> {
        let mut guard = self.async_fd.readable().await?;
        let events = self.inner.drain()?;
        guard.clear_ready();
        Ok(events)
    }
}
