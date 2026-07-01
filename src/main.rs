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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut state = ExtDataControlStream::new()?;
    state.offer_text(String::from("BOO"))?;

    'outer: loop {
        for event in state.read()? {
            println!("{event:?}");

            if let ExtDataControlEvent::Received(text) = event
                && text == "EXIT"
            {
                break 'outer;
            }
        }
    }

    state.cleanup();

    Ok(())
}
