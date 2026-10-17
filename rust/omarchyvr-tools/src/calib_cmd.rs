//! `omarchyvr calib [out.xml]`: read the factory calibration document over IF0 (84 x `58 07 0a 00 01 AL AH 10`), inflate it,
//! write the XML and diff it against `analysis/headset-calibration.xml` (port of `linux/pimax_calib.py`).
//! Offline mode: `--blob FILE` inflates a saved 1344-byte blob instead of talking to the device.
//! `--save-blob FILE` also stores the raw blob.
//! `--save-config` writes the XML to `~/.config/omarchyvr/headset-calibration.xml` (mode 600, atomically, only after the
//! blob inflated) for the app's Room setup; it only READS from the headset: 84 allowlisted `read_calib_block` requests
//! (`58 07 0a 00 01 AL AH 10`, rust/pimax-device/devices/dream-air/device.toml), no wake, no keepalive.
use crate::args::Args;
use crate::{CliError, CliResult};
use pimax::{Config, NoopHandler, Session};
use pimax_proto::calib;
use std::time::Duration;

fn reference() -> Option<Vec<u8>> {
    [
        "analysis/headset-calibration.xml",
        "../analysis/headset-calibration.xml",
    ]
    .iter()
    .map(std::path::PathBuf::from)
    .chain(
        std::env::var_os("HOME")
            .into_iter()
            .flat_map(|h| omarchyvr_config::paths::headset_calibration_candidates(std::path::Path::new(&h))),
    )
    .find_map(|p| std::fs::read(p).ok())
}

/// Entry point.
pub fn run(mut a: Args) -> CliResult {
    let blob_in = a.opt("--blob")?;
    let save_blob = a.opt("--save-blob")?;
    let save_config = a.flag("--save-config");
    a.no_unknown_flags()?;
    let out: String = if save_config {
        omarchyvr_config::paths::headset_calibration_target(std::path::Path::new(&std::env::var("HOME").map_err(|_| CliError::new("HOME is not set"))?)).display().to_string()
    } else {
        a.pos(0)?.unwrap_or_else(|| "headset-calibration-linux.xml".into())
    };
    let blob = match blob_in {
        Some(p) => std::fs::read(p)?,
        None => {
            let s = Session::open(Config::passive(), NoopHandler)?;
            s.wait_connected(Duration::from_secs(10))
                .map_err(|_| CliError::new("IF0 not found"))?;
            s.read_calibration_blob(Duration::from_millis(1000))?
        }
    };
    if let Some(p) = save_blob {
        std::fs::write(p, &blob)?;
    }
    let zlen = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]);
    let xml = calib::inflate_blob(&blob).map_err(|e| CliError::new(e.to_string()))?;
    if save_config {
        write_private(std::path::Path::new(&out), &xml)?;
    } else {
        std::fs::write(&out, &xml)?;
    }
    println!(
        "read {} B, zlib {zlen} B -> xml {} B -> {out}",
        blob.len(),
        xml.len()
    );
    if save_config {
        return Ok(());
    }
    if let Some(r) = reference() {
        if r == xml {
            println!("IDENTICAL to analysis/headset-calibration.xml");
        } else {
            println!("DIFFERS from Windows capture:");
            let (ra, xa) = (
                String::from_utf8_lossy(&r).into_owned(),
                String::from_utf8_lossy(&xml).into_owned(),
            );
            let (rl, xl): (Vec<_>, Vec<_>) = (ra.lines().collect(), xa.lines().collect());
            for i in 0..rl.len().max(xl.len()) {
                if rl.get(i) != xl.get(i) {
                    println!(
                        "@@ line {} @@\n-{}\n+{}",
                        i + 1,
                        rl.get(i).unwrap_or(&""),
                        xl.get(i).unwrap_or(&"")
                    );
                }
            }
        }
    }
    Ok(())
}

/// Write `data` to `path` with mode 600 through a temporary file in the same directory (the factory calibration is
/// private per-device data).
fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("xml.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}
