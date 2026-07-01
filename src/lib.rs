#![doc = include_str!("../README.md")]
#![warn(trivial_casts)]
#![warn(trivial_numeric_casts)]
#![warn(unused_qualifications)]
#![warn(deprecated_in_future)]
#![warn(missing_docs)]
#![warn(unused_lifetimes)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]
#![warn(clippy::indexing_slicing)]
#![warn(clippy::arithmetic_side_effects)]
#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]
#![warn(clippy::std_instead_of_alloc)]
#![warn(clippy::std_instead_of_core)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::option_if_let_else)]

mod app_connection;
mod epoll;
mod errors;
mod ext_data_control_stream;
mod mime_types;
mod offer_seq;
mod reader;
mod rw_stream;
mod wl_event;
mod wl_events_stream;
mod writer;

pub use epoll::EpollError;
pub use errors::{ExtDataControlConnectError, ExtDataControlReadError};
pub use ext_data_control_stream::{ExtDataControlEvent, ExtDataControlStream};
