//! The three USB operations the client needs. Implemented by [`crate::usbfs::UsbFs`] and by [`crate::fake::FakeChip`].

use std::io;
use std::time::Duration;

/// Minimal synchronous USB device interface.
pub trait Transport {
    /// Control transfer. `setup` is the 8-byte setup packet; `out` is the data stage of an OUT request (its length
    /// must equal `wLength`). Returns the data stage of an IN request, empty for OUT requests.
    fn control(&mut self, setup: [u8; 8], out: &[u8], timeout: Duration) -> io::Result<Vec<u8>>;

    /// Bulk OUT transfer of exactly `data` on endpoint `ep`.
    fn bulk_out(&mut self, ep: u8, data: &[u8], timeout: Duration) -> io::Result<()>;

    /// Bulk IN transfer of at most `max_len` bytes; `Ok(None)` when nothing arrived before the timeout.
    fn bulk_in(&mut self, ep: u8, max_len: usize, timeout: Duration)
        -> io::Result<Option<Vec<u8>>>;
}
