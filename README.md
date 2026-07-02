## `wl-data-control-protocol-evt`

Implementation of `ext-data-control` Wayland protocol that is:

1. pollable
2. event based

### API

Full examples can be found in [poll.rs](/examples/poll.rs) and [tokio.rs](/examples/tokio.rs).

Here's a quick `poll`-based API overview:

```rust,no_run
use wl_data_control_protocol_evt::{ExtDataControlEvent, ExtDataControlStream};

let mut stream = ExtDataControlStream::new().unwrap();
let fd = stream.as_fd();

loop {
    poll([fd], IN)?;

    // read
    let events = stream.drain()?;
    for event in events {
        if let ExtDataControlEvent::Received(text) = event {
            println!("copied {text}")
        }
    }

    // or write
    stream.offer_text("try pasting this").unwrap()
}
```

For async IO you can use something like [AsyncFd](https://docs.rs/tokio/latest/tokio/io/unix/struct.AsyncFd.html):

```rust,no_run
let stream = ExtDataControlStream::new()?;
let async_fd = AsyncFd::new(stream.as_raw_fd())?;

loop {
    let mut guard = async_fd.readable().await?;
    let events = stream.drain()?;
    guard.clear_ready();

    // process `events`
}
```
