//! Firmware log (IF3, bridge EP 0x81): ASCII chunks of up to 64 bytes, split into lines.

/// Maximum line length before a forced flush.
pub const LINE_MAX: usize = 512;

/// Incremental line splitter. NUL and CR are dropped; a line is flushed on LF or when the buffer is full.
#[derive(Clone)]
pub struct LineSplitter {
    buf: [u8; LINE_MAX],
    len: usize,
}

impl Default for LineSplitter {
    fn default() -> Self {
        Self::new()
    }
}

impl LineSplitter {
    /// Empty splitter.
    pub const fn new() -> Self {
        Self {
            buf: [0; LINE_MAX],
            len: 0,
        }
    }

    /// Drop a partial line.
    pub fn reset(&mut self) {
        self.len = 0;
    }

    /// Feed a chunk, calling `emit` once per non-empty line (bytes, no newline).
    pub fn push(&mut self, data: &[u8], mut emit: impl FnMut(&[u8])) {
        for &c in data {
            if c == 0 || c == b'\r' {
                continue;
            }
            if c == b'\n' || self.len >= LINE_MAX - 1 {
                if self.len > 0 {
                    emit(&self.buf[..self.len]);
                }
                self.len = 0;
                if c == b'\n' {
                    continue;
                }
            }
            self.buf[self.len] = c;
            self.len += 1;
        }
    }
}
