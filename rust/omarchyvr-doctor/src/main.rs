//! `omarchyvr-doctor [--json] [--fix-plan]` and `omarchyvr-doctor info --json`. Read-only.
use omarchyvr_doctor::{info, run_checks, Opts, RealHost};
use std::process::ExitCode;

const USAGE: &str = "usage: omarchyvr-doctor [--json] [--fix-plan]\n       omarchyvr-doctor info [--json]\n\n\
  (default)    run all health checks, print a table; exit 1 if any check failed\n\
  --json       machine readable output\n\
  --fix-plan   print the commands that would fix each problem (never runs them)\n\
  info         headset / GPU / connector / profile description (always JSON)\n\n\
Environment: PIMAX_MONADO_BUILD, PIMAX_LINUX_DIR, PIMAX_SYSFS_ROOT, PIMAX_PROC_ROOT, PIMAX_DEV_ROOT, NVIDIA_SRC_ROOT.\n";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = |k: &str| std::env::var(k).ok();
    let opts = Opts::from_env(&env);
    let host = RealHost::from_env();
    let mut json = false;
    let mut plan = false;
    let mut is_info = false;
    for a in &args {
        match a.as_str() {
            "--json" => json = true,
            "--fix-plan" => plan = true,
            "info" => is_info = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if is_info {
        print!("{}", info::gather(&host, &opts).render());
        return ExitCode::SUCCESS;
    }
    let report = run_checks(&host, &opts);
    if plan {
        print!("{}", report.fix_plan());
    } else if json {
        print!("{}", report.to_json().render());
    } else {
        print!("{}", report.render());
    }
    if report.has_fail() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
