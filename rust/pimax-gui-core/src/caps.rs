//! `getcap` output (LESSONS row 7: every relink of monado-service drops `cap_sys_nice`; the GPU priority request then
//! fails and frames stutter). Both output styles are understood:
//! libcap >= 2.4x `/path cap_sys_nice=ep` and the old `/path = cap_sys_nice+ep`.
use crate::cmd::Cmd;
use std::path::{Path, PathBuf};

/// Does the `getcap <path>` output grant `cap_sys_nice` as effective and permitted?
pub fn has_sys_nice(getcap_output: &str) -> bool {
    getcap_output.lines().any(|l| {
        // drop the path (first word) and an optional "=" separator of the old format
        let mut words = l.split_whitespace();
        if words.next().is_none() {
            return false;
        }
        let spec: Vec<&str> = words.filter(|w| *w != "=").collect();
        spec.iter().any(|clause| clause_grants(clause, "cap_sys_nice"))
    })
}

/// One clause such as `cap_net_raw,cap_sys_nice=ep` or `cap_sys_nice+eip`.
fn clause_grants(clause: &str, cap: &str) -> bool {
    let Some(op_at) = clause.find(['=', '+']) else {
        return false;
    };
    let (caps, flags) = clause.split_at(op_at);
    let flags = &flags[1..];
    // an empty list (`/path =ep`) or `all` means every capability
    let names_cap = caps.is_empty() || caps.split(',').any(|c| c == cap || c == "all");
    names_cap && flags.contains('e') && flags.contains('p')
}

/// The exact fix the app shows (root, run by the user with pkexec or in a terminal).
pub fn setcap_command(service: &Path) -> String {
    format!("sudo setcap cap_sys_nice+ep {}", crate::cmd::quote(&service.display().to_string()))
}

/// The setcap the one-click fix runs (absolute: a shim earlier in PATH must not run as root).
pub const SETCAP: &str = "/usr/bin/setcap";

/// The one-click fix of the Home page and of Play: `/usr/bin/pkexec /usr/bin/setcap cap_sys_nice+ep <service>` (one
/// polkit password prompt, nothing installed). `service` must come from [`check_service_path`]; the safety rules
/// ([`crate::safety::check`]) accept exactly this shape.
pub fn pkexec_setcap(service: &Path) -> Cmd {
    Cmd {
        program: crate::safety::PKEXEC.into(),
        args: vec![SETCAP.into(), "cap_sys_nice+ep".into(), service.display().to_string()],
        root: true,
        ..Default::default()
    }
}

/// Resolve a monado-service path for the one-click setcap and refuse anything else: after resolving symlinks it must
/// be a regular file named `monado-service` under `<home>/src/monado*/` (the user's own Monado builds). Returns the
/// resolved absolute path (the one pkexec gets).
pub fn check_service_path(service: &Path, home: &Path) -> Result<PathBuf, String> {
    let resolved = std::fs::canonicalize(service).map_err(|e| format!("{}: {e}", service.display()))?;
    let meta = std::fs::symlink_metadata(&resolved).map_err(|e| format!("{}: {e}", resolved.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", resolved.display()));
    }
    if resolved.file_name().and_then(|n| n.to_str()) != Some("monado-service") {
        return Err(format!("{} is not named monado-service", resolved.display()));
    }
    let src_root = pimax_config::paths::src_dir(home);
    let src = std::fs::canonicalize(&src_root).map_err(|e| format!("{}: {e}", src_root.display()))?;
    let rel = resolved.strip_prefix(&src).ok();
    let top_ok = rel
        .and_then(|r| r.components().next())
        .and_then(|c| c.as_os_str().to_str())
        .is_some_and(|top| top.starts_with("monado"));
    if !top_ok || rel.is_none_or(|r| r.components().count() < 2) {
        return Err(format!("{} is not under {}/monado*/", resolved.display(), src.display()));
    }
    if resolved.to_str().is_none_or(|s| s.chars().any(char::is_control)) {
        return Err(format!("{} has characters the fix does not accept", resolved.display()));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_one_click_fix_only_takes_a_monado_service_under_src_monado() {
        let home = std::env::temp_dir().join(format!("pimax-caps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let svc_dir = home.join("src/monado-ht/build-ht/src/xrt/targets/service");
        std::fs::create_dir_all(&svc_dir).unwrap();
        let svc = svc_dir.join("monado-service");
        std::fs::write(&svc, "elf").unwrap();
        let ok = check_service_path(&svc, &home).unwrap();
        assert_eq!(ok, std::fs::canonicalize(&svc).unwrap());
        let c = pkexec_setcap(&ok);
        assert_eq!(c.program, "/usr/bin/pkexec");
        assert_eq!(c.args[..2], ["/usr/bin/setcap".to_string(), "cap_sys_nice+ep".to_string()]);
        assert_eq!(crate::safety::check(&c), Ok(()));
        // a symlinked build directory resolves to the real file
        let link = home.join("src/monado-link");
        std::os::unix::fs::symlink(&svc_dir, &link).unwrap();
        assert_eq!(check_service_path(&link.join("monado-service"), &home).unwrap(), ok);
        // wrong name, wrong place, a directory, missing
        let other = svc_dir.join("python3");
        std::fs::write(&other, "x").unwrap();
        assert!(check_service_path(&other, &home).is_err());
        let elsewhere = home.join("src/wayvr/monado-service");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, "x").unwrap();
        assert!(check_service_path(&elsewhere, &home).is_err(), "not under src/monado*");
        let top = home.join("src/monado-service");
        std::fs::write(&top, "x").unwrap();
        assert!(check_service_path(&top, &home).is_err(), "directly in src/");
        let dir = home.join("src/monado-x/monado-service");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(check_service_path(&dir, &home).is_err(), "a directory");
        assert!(check_service_path(&svc_dir.join("nope/monado-service"), &home).is_err());
        // a symlink named monado-service that points elsewhere
        let evil = home.join("src/monado-evil");
        std::fs::create_dir_all(&evil).unwrap();
        std::os::unix::fs::symlink(&other, evil.join("monado-service")).unwrap();
        assert!(check_service_path(&evil.join("monado-service"), &home).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn modern_and_old_formats() {
        let p = "/home/u/src/monado-eye/build/src/xrt/targets/service/monado-service";
        assert!(has_sys_nice(&format!("{p} cap_sys_nice=ep\n")));
        assert!(has_sys_nice(&format!("{p} = cap_sys_nice+ep\n")));
        assert!(has_sys_nice(&format!("{p} cap_net_raw,cap_sys_nice=eip\n")));
        assert!(has_sys_nice(&format!("{p} =ep\n")), "all capabilities");
    }

    #[test]
    fn missing_or_partial_capability_is_not_enough() {
        let p = "/x/monado-service";
        assert!(!has_sys_nice(""), "getcap prints nothing for a file without capabilities");
        assert!(!has_sys_nice(&format!("{p} cap_sys_nice=p\n")), "permitted only");
        assert!(!has_sys_nice(&format!("{p} cap_net_admin=ep\n")));
        assert!(!has_sys_nice("Failed to get capabilities of file '/x' (No such file or directory)\n"));
        assert_eq!(setcap_command(std::path::Path::new(p)), "sudo setcap cap_sys_nice+ep /x/monado-service");
    }
}
