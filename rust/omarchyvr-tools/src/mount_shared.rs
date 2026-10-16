//! `omarchyvr mount-shared`: prints (never runs) the command that mounts the shared NTFS drive, and says whether it is
//! mounted already. The drive is a personal setting: `shared_drive_uuid` in `~/.config/omarchyvr/paths.toml` (or
//! `$PIMAX_SHARED_UUID`); without it the command says so and the app shows no shared-drive check.
use crate::args::Args;
use crate::{CliError, CliResult};
use std::path::PathBuf;

/// Filesystem UUID of the shared drive, if this machine has one configured (no default).
pub fn configured_uuid(home: &std::path::Path) -> Option<String> {
    omarchyvr_config::paths::shared_drive_uuid(home)
}
/// Where it is mounted.
pub const SHARED_TARGET: &str = "/mnt/shared";

/// The root command to mount the drive: the in-kernel ntfs3 driver, files owned by the user (same options as the
/// mount that is normally active: `uid=1000,gid=1000,acl,iocharset=utf8,prealloc`).
pub fn mount_command(uuid: &str, uid: u32, gid: u32) -> String {
    format!(
        "sudo mkdir -p {SHARED_TARGET} && sudo mount -t ntfs3 -o uid={uid},gid={gid},acl,iocharset=utf8,prealloc UUID={uuid} {SHARED_TARGET}"
    )
}

/// `(source, fstype)` of the mount at `target` in the text of `/proc/mounts` (the last entry wins, like the kernel's
/// view of stacked mounts).
pub fn mounted_at(proc_mounts: &str, target: &str) -> Option<(String, String)> {
    proc_mounts
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (src, mp, ty) = (f.next()?, f.next()?, f.next()?);
            // spaces in a mount point are written as \040
            (mp.replace("\\040", " ") == target).then(|| (src.to_string(), ty.to_string()))
        })
        .next_back()
}

/// Entry point.
pub fn run(a: Args) -> CliResult {
    if matches!(
        a.0.first().map(String::as_str),
        Some("-h" | "--help" | "help")
    ) {
        println!("usage: omarchyvr mount-shared\n  prints the mount command for the drive set as shared_drive_uuid in ~/.config/omarchyvr/paths.toml on {SHARED_TARGET} (does not run it) and whether it is mounted");
        return Ok(());
    }
    a.no_unknown_flags()?;
    if let Some(x) = a.0.first() {
        return Err(CliError::usage(format!("unexpected argument {x}")));
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let Some(uuid) = configured_uuid(&home) else {
        println!("no shared drive set: add shared_drive_uuid = \"<UUID>\" to ~/.config/omarchyvr/paths.toml (see `ls /dev/disk/by-uuid`)");
        return Ok(());
    };
    let proc_root = std::env::var_os("PIMAX_PROC_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/proc".into());
    let mounts = std::fs::read_to_string(proc_root.join("mounts"))
        .map_err(|e| CliError::new(format!("cannot read /proc/mounts: {e}")))?;
    // SAFETY: getuid/getgid take no arguments and cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    match mounted_at(&mounts, SHARED_TARGET) {
        Some((src, ty)) => {
            println!("mounted: {SHARED_TARGET} ({src}, {ty})");
            println!("(to mount it again after an unmount, as root:)");
        }
        None => println!("NOT mounted: {SHARED_TARGET}. Run this as root:"),
    }
    println!("{}", mount_command(&uuid, uid, gid));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_uses_the_uuid_target_and_owner() {
        let c = mount_command("0123456789ABCDEF", 1000, 1001);
        assert!(c.contains("UUID=0123456789ABCDEF /mnt/shared"));
        assert!(c.contains("-t ntfs3") && c.contains("uid=1000,gid=1001"));
        assert!(c.starts_with("sudo mkdir -p /mnt/shared && sudo mount"));
    }

    #[test]
    fn mounted_detection() {
        let m = "/dev/nvme0n1p2 / btrfs rw 0 0\n/dev/nvme1n1p2 /mnt/shared ntfs3 rw,uid=1000 0 0\n/dev/x /mnt/shared2 ext4 rw 0 0\n";
        assert_eq!(
            mounted_at(m, "/mnt/shared"),
            Some(("/dev/nvme1n1p2".into(), "ntfs3".into()))
        );
        assert_eq!(mounted_at(m, "/mnt"), None);
        assert_eq!(mounted_at("", "/mnt/shared"), None);
        assert_eq!(mounted_at("garbage\n", "/mnt/shared"), None);
    }
}
