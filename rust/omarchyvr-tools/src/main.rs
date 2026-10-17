use omarchyvr_tools::args::Args;
use omarchyvr_tools::{
    calib_cmd, controllers, imu, info, keepalived, logger, logtail, mount_shared, probe, video_on,
    CliError, CliResult,
};

const USAGE: &str = "omarchyvr - Pimax Dream Air userspace tool (Rust port of the python scripts in linux/)

usage: omarchyvr <subcommand> [options]
  keepalived [--no-wake] [--rate-file P]   daemon: wake + 213 ms keepalive, drain IF1-IF3, auto-reconnect
  probe [seconds] [--passive]              wake/keepalive (unless --passive), identity reads, IMU/log summary
  imu [--json] [--rate HZ] [--no-calib] [--calib F] [--auto-bias S] [--still N] [--seconds T]
      [--kp K] [--ki K] [--axis-mode 0|1]  Mahony orientation from IF2 (read-only); axis mode 1 default
  controllers [seconds] [--raw]            attach + decoded controller reports
  controllers unpair left|right            unpair one controller (58 03 05 SS)
  controllers buzz left|right [--amp 25] [--ms 300]  test vibration (58 09 07 SS AMP DUR); through the
                                           running VR session if one runs
  controllers use when-held|always|off     when the running session shows the controllers (PIMAX_CTRL_INHAND)
  controllers status                       in hand / put down, per controller (running session)
  calib [out.xml] [--blob F] [--save-blob F]   read the factory calibration (84 reads), inflate, diff
  video-on [fps=72] [seconds=40]           switch the panel on (changes headset state!) and tail logs
  logtail [seconds=30] [regex]             passive firmware + bridge log tail
  info [--controllers]                     serial, version, interfaces
  vr start|stop|status|steam-options       one-click VR: preflight + monado-service (omarchyvr vr help)
  desk start|stop|status [--no-passthrough] [--eye]   desk mode: Monado + passthrough (main app) + WayVR overlay (omarchyvr desk help)
  auto status|events|pause|resume|wake|sleep|pair left|right|set-mode [desk|game] [--runtime monado|steamvr] [--preset P]
                                           control the auto-mode daemon omarchyvr-autod (docs/AUTO-MODE.md)
  record start|stop|status [--eyes left|both] [--fps 30|60|72] [--quality low|medium|high]
                                           record what the headset shows to ~/Videos/pimax (omarchyvr record help)
  record tracking start|stop|status [--scenario S] [--seconds N]   cameras + logs into one folder per take
  mount-shared                             print the mount command of the shared NTFS drive, say if it is mounted";

fn run() -> CliResult {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() || matches!(argv[0].as_str(), "-h" | "--help" | "help") {
        println!("{USAGE}");
        return if argv.is_empty() {
            Err(CliError::usage(""))
        } else {
            Ok(())
        };
    }
    let sub = argv.remove(0);
    logger::init();
    let a = Args(argv);
    match sub.as_str() {
        "keepalived" => keepalived::run(a),
        "probe" => probe::run(a),
        "imu" => imu::run(a),
        "controllers" => controllers::run(a),
        "calib" => calib_cmd::run(a),
        "video-on" => video_on::run(a),
        "logtail" => logtail::run(a),
        "info" => info::run(a),
        "vr" => omarchyvr_tools::vr::run(a),
        "desk" => omarchyvr_tools::vr::desk::run(a),
        "mount-shared" => mount_shared::run(a),
        "auto" => auto(a),
        "record" => match omarchyvr_record::cli::main(&a.0) {
            0 => Ok(()),
            code => Err(CliError { msg: String::new(), code }),
        },
        other => Err(CliError::usage(format!(
            "unknown subcommand {other}\n{USAGE}"
        ))),
    }
}

/// `omarchyvr auto ...`: a client of omarchyvr-autod's control socket (one JSON line in, one out; `events` streams).
fn auto(a: Args) -> CliResult {
    use omarchyvr_autod::{client, ipc};
    let line = client::request_line(&a.0).map_err(CliError::usage)?;
    let sock = ipc::socket_path();
    if a.0.first().map(String::as_str) == Some("pair") {
        return client::pair_cli(&sock, &a.0[1], &mut |l| println!("{l}")).map_err(CliError::new);
    }
    if a.0.first().map(String::as_str) == Some("events") {
        client::events(&sock, |l| {
            println!("{l}");
            true
        })
        .map_err(CliError::new)
    } else {
        println!("{}", client::request(&sock, &line).map_err(CliError::new)?);
        Ok(())
    }
}

fn main() {
    // run from a WayVR button (Exit Work): the user's own XDG directories, not WayVR's isolated ones
    omarchyvr_config::paths::restore_user_xdg();
    omarchyvr_config::migrate_home(); // one time: ~/.config/omarchyvr etc. -> omarchyvr
    if let Err(e) = run() {
        if !e.msg.is_empty() {
            eprintln!("omarchyvr: {}", e.msg);
        }
        std::process::exit(e.code);
    }
}
