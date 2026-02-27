use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

static REMOTE_CONTROL_REQUESTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn request_remote_control() {
    REMOTE_CONTROL_REQUESTED.store(true, Ordering::SeqCst);
}

pub fn take_remote_control_request() -> bool {
    REMOTE_CONTROL_REQUESTED.swap(false, Ordering::SeqCst)
}
