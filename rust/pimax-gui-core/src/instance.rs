//! One Pimax app per user session: an advisory `flock` on `$XDG_RUNTIME_DIR/pimax-setup.lock`, which holds the pid
//! of the process that owns it. The lock goes away with that process, whatever way it ends.
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// The held lock (released when dropped or when the process ends).
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    /// Lock file.
    pub path: PathBuf,
}

/// `$XDG_RUNTIME_DIR/pimax-setup.lock` (or `/tmp/pimax-setup-<uid>.lock`).
pub fn lock_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    match env("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d).join("pimax-setup.lock"),
        // SAFETY: getuid(2) has no arguments and cannot fail.
        None => PathBuf::from(format!("/tmp/pimax-setup-{}.lock", unsafe { libc::getuid() })),
    }
}

/// Take the lock. `Err(Some(pid))`: another instance holds it; `Err(None)`: the lock file cannot be used (the app
/// then runs without the single-instance guard).
pub fn acquire(path: &Path) -> Result<InstanceLock, Option<u32>> {
    let mut f = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).map_err(|_| None)?;
    // SAFETY: flock(2) on a valid descriptor of a file this process opened.
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if r != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            let mut s = String::new();
            let _ = f.read_to_string(&mut s);
            return Err(s.trim().parse().ok().or(Some(0)));
        }
        return Err(None);
    }
    let _ = f.set_len(0);
    let _ = f.seek(SeekFrom::Start(0));
    let _ = write!(f, "{}", std::process::id());
    let _ = f.flush();
    Ok(InstanceLock {
        _file: f,
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_sees_the_owner() {
        let p = std::env::temp_dir().join(format!("pimax-instance-{}.lock", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let a = acquire(&p).expect("first");
        // flock locks belong to the open file description: a second open in the same process conflicts too
        match acquire(&p) {
            Err(Some(pid)) => assert_eq!(pid, std::process::id()),
            other => panic!("{other:?}"),
        }
        drop(a);
        let b = acquire(&p).expect("free again");
        drop(b);
        assert_eq!(lock_path(&|k| (k == "XDG_RUNTIME_DIR").then(|| "/run/user/7".to_string())), PathBuf::from("/run/user/7/pimax-setup.lock"));
        let _ = std::fs::remove_file(p);
    }
}
