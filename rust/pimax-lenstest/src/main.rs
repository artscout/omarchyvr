use pimax_lenstest::cli::{self, Command};
use pimax_lenstest::geometry::Fov;
use pimax_lenstest::patterns::Params;
use pimax_lenstest::{dump, preview, report, xr};
use std::process::ExitCode;

fn main() -> ExitCode {
    // `pimax-lenstest list | head` must end quietly, not with a panic about the closed pipe
    // SAFETY: restoring the default SIGPIPE disposition before anything else runs.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Err(e) => {
            eprintln!("pimax-lenstest: {e}\n\n{}", cli::USAGE);
            ExitCode::from(2)
        }
        Ok(Command::Help) => {
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::List) => {
            let p = Params::new(
                3552,
                3840,
                Fov::from_half_angles_deg(50.0, 50.0, 44.5, 44.5),
                0,
            );
            print!("{}", report::list_text(&p));
            ExitCode::SUCCESS
        }
        Ok(Command::Config { lens_conf }) => {
            print!(
                "{}",
                report::lens_report(&report::load_lens(lens_conf.as_deref()), true)
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Preview { dir, w, h, fov }) => match preview::write_all(&dir, w, h, fov, 5.0) {
            Ok(files) => {
                println!("{} pictures written to {}", files.len(), dir.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("pimax-lenstest: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::BenchSwim { w, h, frames }) => {
            print!("{}", pimax_lenstest::swimbench::run(w, h, frames));
            ExitCode::SUCCESS
        }
        Ok(Command::Compare { capture, sim }) => {
            match pimax_lenstest::compare::compare_dirs(&capture, &sim) {
                Ok(rows) if rows.is_empty() => {
                    eprintln!(
                        "pimax-lenstest: no picture of {} has a counterpart in {}",
                        capture.display(),
                        sim.display()
                    );
                    ExitCode::FAILURE
                }
                Ok(rows) => {
                    const LIMIT: f64 = 0.01;
                    print!("{}", pimax_lenstest::compare::table(&rows, LIMIT));
                    if rows.iter().all(|r| r.rms <= LIMIT) {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(3)
                    }
                }
                Err(e) => {
                    eprintln!("pimax-lenstest: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Ok(Command::Simulate {
            dir,
            lens_conf,
            div,
        }) => {
            let info = report::load_lens(lens_conf.as_deref());
            println!(
                "lens.conf: {} ({})",
                info.path.display(),
                if info.found {
                    "found"
                } else {
                    "not found, defaults"
                }
            );
            match preview::simulate_all(&dir, &info.lens, div, 1) {
                Ok(files) => {
                    println!("{} pictures written to {}", files.len(), dir.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("pimax-lenstest: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Ok(Command::Dump2Png {
            raw,
            out,
            down,
            rot_left,
            rot_right,
        }) => match dump::convert(&raw, &out, down, rot_left, rot_right) {
            Ok(c) => {
                println!(
                    "{}: frame {} {}x{} VkFormat {} fnv1a {:016x} -> {}",
                    raw.display(),
                    c.info.frame,
                    c.info.w,
                    c.info.h,
                    c.info.vk_format,
                    c.checksum,
                    out.display()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("pimax-lenstest: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::DumpDiff { a, b, png, down }) => {
            match dump::diff_files(&a, &b, png.as_deref(), down) {
                Ok(st) => {
                    if st.identical() {
                        println!(
                            "IDENTICAL: {}x{} = {} pixels, every pixel equal",
                            st.w, st.h, st.total
                        );
                        ExitCode::SUCCESS
                    } else {
                        println!(
                        "DIFFERENT: {} of {} pixels differ ({:.4}%), largest difference {} of 1023 (10-bit codes), mean {:.3}, first at {:?}, box {:?}",
                        st.differing,
                        st.total,
                        100.0 * st.differing as f64 / st.total as f64,
                        st.max_diff,
                        st.sum_abs as f64 / (3.0 * st.differing as f64),
                        st.first,
                        st.bbox
                    );
                        ExitCode::from(3)
                    }
                }
                Err(e) => {
                    eprintln!("pimax-lenstest: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Ok(Command::Run(opts)) => match xr::run(opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("pimax-lenstest: error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}
