## `wl-data-control-protocol-evt`

Implementation of `ext-data-control` Wayland protocol that is:

1. pollable
2. event based

### API

A full example can be found in [demo.rs](/examples/demo.rs), here's a quick API overview:

```rust,no_run
use wl_data_control_protocol_evt::{ExtDataControlEvent, ExtDataControlStream};

let mut stream = ExtDataControlStream::new().unwrap();
let fd = stream.as_fd();

loop {
    poll(fd, IN)?;

    // read
    let events = stream.read();
    for event in events {
        if let ExtDataControlEvent::Received(text) = event {
            println!("copied {text}")
        }
    }

    // or write
    stream.offer_text("try pasting this").unwrap()
}
```
