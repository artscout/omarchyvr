//! The app shows (and its crash reports name) the git commit and the build date it was built from.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let o = Command::new("git").args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn main() {
    // packagers without a git checkout set PIMAX_GIT_HASH
    let hash = std::env::var("PIMAX_GIT_HASH")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            let h = git(&["rev-parse", "--short=9", "HEAD"])?;
            let dirty = git(&["status", "--porcelain", "--untracked-files=no", "--", "."]).is_some_and(|s| !s.is_empty());
            Some(if dirty { format!("{h}+") } else { h })
        })
        .unwrap_or_else(|| "unknown".into());
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    let days = (secs / 86400) as i64;
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    println!("cargo:rustc-env=PIMAX_GIT_HASH={hash}");
    println!("cargo:rustc-env=PIMAX_BUILD_DATE={y:04}-{m:02}-{d:02}");
    println!("cargo:rerun-if-env-changed=PIMAX_GIT_HASH");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    for f in ["../../.git/HEAD", "../../.git/index"] {
        println!("cargo:rerun-if-changed={f}");
    }
}
