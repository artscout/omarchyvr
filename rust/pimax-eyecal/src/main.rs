use pimax_eyecal::procedure::{Mode, Procedure, Settings};
use pimax_eyecal::{report, sim, xrapp};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
pimax-eyecal: eye-tracking calibration and accuracy test for the Pimax Dream Air (Monado eye build, PIMAX_EYE=1)

  pimax-eyecal [--pattern grid|pimax] [--points 9|13] [--pursuit] [--recenter] [--flip-x 0|1] [--flip-y 0|1] [--head-limit DEG]
               [--res-div N] [--calibration PATH] [--no-raw-log]
  pimax-eyecal --simulate [--seed N] [--points 9|13] [--pursuit] [--calibration PATH]
  pimax-eyecal --refit RUN.json [--write PATH] [--skip-s S] [--gate-mm MM] [--ivt] [--pupil]

Put the headset on, keep the head still and follow the dots with the eyes only. 9 (or 13) calibration targets
within ±15 deg horizontally / ±12 deg vertically, then 10 different validation targets; the accuracy shown is that
of the validation targets. Then: A live cursor, menu/Y save to ~/.config/omarchyvr/eye-calibration.toml (the old file
is kept as .bak-<utc>), S check through the runtime (what OpenXR apps such as WayVR get), P smooth pursuit.

  --frames FILE     the eye chip start list of a headset without a built-in one (Crystal Super: made by
                    `pimax-eye frames`), with its own profile upload, as `omarchyvr eyed --frames` starts it
  --pattern P       grid (default: 9 or 13 targets on a dark background) or pimax (Pimax Play's path: a green dot
                    travelling centre, corners, centre on black, then left, right, centre, up, down on white; the fit
                    also tries the pupil-size terms)
  --points N        9 (default) or 13 calibration targets (13 also fit a quadratic model)
  --pursuit         run the smooth-pursuit test after the validation
  --recenter        quick 5-target update of the drift offsets of the existing calibration
  --flip-x 0|1      device-to-head x conversion (default PIMAX_EYE_FLIP_X, else 1: measured 14 Oct); a mirrored
                    axis is detected by the fit and the tracker restarted with the other setting
  --head-limit DEG  head rotation that restarts a target (default 2.5)
  --res-div N       render at 1/N of the recommended resolution
  --calibration P   write the calibration to P instead of ~/.config/omarchyvr/eye-calibration.toml
  --no-raw-log      do not write ~/.local/share/omarchyvr/eye-calibration/eyecal-<utc>-raw.csv
  --simulate        run the whole procedure on a simulated wearer and chip (no headset), print the report

  --refit RUN.json  fit the calibration targets of a saved run again (RUN.json and RUN-raw.csv, no headset) with the
                    sample-level fit with origin terms, and print the validation accuracy of the old and the new fit on
                    the run's own validation targets; the new calibration goes to RUN-refit.toml (never to the config
                    directory by itself: add --write PATH, the old file is kept as .bak-<utc>)
  --skip-s S        --refit: seconds dropped at the start of every window (default 0.25)
  --gate-mm MM      --refit: eye origin farther than this from its usual place is dropped (default 5, 0 = off)
  --pupil           --refit: also offer models with a pupil-diameter term (taken only when clearly better)
  --ivt             --refit: only the longest velocity-threshold fixation of every window (default: all steady samples)

The EyeChip is opened directly (no other gaze app may run: WayVR holds it through the runtime); results go to
~/.local/share/omarchyvr/eye-calibration/eyecal-<utc>.json.
Keys: space/a next, r redo, y save, p pursuit, c cursor, s runtime check, q quit.
";

fn flag_value(args: &mut Vec<String>, name: &str) -> Result<Option<String>, String> {
    if let Some(i) = args.iter().position(|a| a == name) {
        if i + 1 >= args.len() {
            return Err(format!("{name} needs a value"));
        }
        let v = args.remove(i + 1);
        args.remove(i);
        return Ok(Some(v));
    }
    Ok(None)
}

fn flag(args: &mut Vec<String>, name: &str) -> bool {
    match args.iter().position(|a| a == name) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    }
}

fn env_flag(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if flag(&mut args, "--help") || flag(&mut args, "-h") {
        print!("{USAGE}");
        return Ok(());
    }
    let mut s = Settings::default();
    if let Some(v) = flag_value(&mut args, "--frames")? {
        // read by the tracker start (also after a restart with other flips); single-threaded here
        std::env::set_var("PIMAX_EYE_FRAMES", v);
    }
    if let Some(v) = flag_value(&mut args, "--pattern")? {
        match v.as_str() {
            "pimax" => s = s.pimax(),
            "grid" => {}
            _ => return Err("--pattern must be pimax or grid".into()),
        }
    }
    if let Some(v) = flag_value(&mut args, "--points")? {
        s.cal_points = match v.as_str() {
            "9" => 9,
            "13" => 13,
            _ => return Err("--points must be 9 or 13".into()),
        };
    }
    s.pursuit = flag(&mut args, "--pursuit");
    let recenter = flag(&mut args, "--recenter");
    s.conv.flip_x = env_flag("PIMAX_EYE_FLIP_X").unwrap_or(true);
    s.conv.flip_y = env_flag("PIMAX_EYE_FLIP_Y").unwrap_or(false);
    if let Some(v) = flag_value(&mut args, "--flip-x")? {
        s.conv.flip_x = v == "1";
    }
    if let Some(v) = flag_value(&mut args, "--flip-y")? {
        s.conv.flip_y = v == "1";
    }
    if let Some(v) = flag_value(&mut args, "--head-limit")? {
        s.head_limit_deg = v.parse().map_err(|_| "--head-limit: a number of degrees")?;
    }
    let res_div: u32 = match flag_value(&mut args, "--res-div")? {
        Some(v) => v.parse().map_err(|_| "--res-div: a number")?,
        None => 1,
    };
    let calibration_path = flag_value(&mut args, "--calibration")?.map(PathBuf::from);
    let raw_log = !flag(&mut args, "--no-raw-log");
    let refit = flag_value(&mut args, "--refit")?;
    let write = flag_value(&mut args, "--write")?.map(PathBuf::from);
    let mut extract = pimax_eyecal::refit::ExtractOpts::default();
    if let Some(v) = flag_value(&mut args, "--skip-s")? {
        extract.skip_s = v.parse().map_err(|_| "--skip-s: a number of seconds")?;
    }
    if let Some(v) = flag_value(&mut args, "--gate-mm")? {
        extract.gate_mm = v.parse().map_err(|_| "--gate-mm: a number of millimetres")?;
    }
    extract.ivt = flag(&mut args, "--ivt");
    extract.pupil = flag(&mut args, "--pupil");
    let simulate = flag(&mut args, "--simulate");
    let seed: u64 = match flag_value(&mut args, "--seed")? {
        Some(v) => v.parse().map_err(|_| "--seed: a number")?,
        None => 1,
    };
    if let Some(a) = args.first() {
        return Err(format!("unexpected argument '{a}'"));
    }
    if let Some(run) = refit {
        let text = pimax_eyecal::refit::refit(&PathBuf::from(run), &pimax_eyecal::refit::RefitOpts { write, extract })?;
        print!("{text}");
        return Ok(());
    }
    if write.is_some() {
        return Err("--write goes with --refit".into());
    }
    if recenter {
        let path = calibration_path
            .clone()
            .or_else(pimax_eye::calib::Calibration::default_path)
            .ok_or("no $HOME")?;
        let cal = pimax_eye::calib::Calibration::load(&path)
            .map_err(|e| format!("--recenter needs an existing calibration: {e}"))?;
        s.conv = cal.conv;
        s.existing = Some(cal);
        s.mode = Mode::Recenter;
    }
    let proc = Procedure::new(s);
    if simulate {
        let out = sim::run(
            proc,
            &sim::SimParams {
                seed,
                ..sim::SimParams::default()
            },
        );
        for l in &out.log {
            println!("{l}");
        }
        let p = &out.procedure;
        print!("{}", report::summary(p).text);
        if let (Some(text), Some(path)) = (&out.saved, &calibration_path) {
            report::write_with_backup(path, text)?;
            println!("simulated calibration written to {}", path.display());
        }
        return Ok(());
    }
    xrapp::run(
        proc,
        xrapp::Opts {
            res_div,
            calibration_path,
            raw_log,
        },
    )
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pimax-eyecal: {e}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}
