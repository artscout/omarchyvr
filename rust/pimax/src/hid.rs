//! Small helpers around hidraw files for tools that read without a [`crate::Session`].
use pimax_proto::{Report, REPORT_SIZE};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Open a hidraw node non-blocking and close-on-exec.
pub fn open(path: &Path, write: bool) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
}

/// Read one report. `Ok(None)` when nothing is queued (EAGAIN); short reads are zero padded.
pub fn read_report(f: &File, out: &mut Report) -> io::Result<Option<usize>> {
    let mut buf = [0u8; REPORT_SIZE];
    match (&*f).read(&mut buf) {
        Ok(0) => Ok(None),
        Ok(n) => {
            *out = buf;
            Ok(Some(n))
        }
        Err(e)
            if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::Interrupted =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Write a command (report id 0 prefix added).
pub fn write_report(f: &File, r: &Report) -> io::Result<()> {
    let buf = pimax_proto::to_write_buf(r);
    (&*f).write_all(&buf)
}

/// True for errors that mean the device is gone and the session should be torn down.
pub fn is_gone(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENODEV)
            | Some(libc::EIO)
            | Some(libc::ENOTCONN)
            | Some(libc::EPIPE)
            | Some(libc::ENOENT)
            | Some(libc::ECONNRESET)
            | Some(libc::EBADF)
    )
}
