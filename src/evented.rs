use crate::epoll::Epoll;

pub(crate) struct Evented {
    epoll: Epoll,
}
