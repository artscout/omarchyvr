use omarchyvr_voicehud::cli::{self, Command};
use omarchyvr_voicehud::{dump, log, preview, runner, sig};
use std::process::ExitCode;

fn main() -> ExitCode {
    // comm of this process: "omarchyvr-voice-hud" cut at 15 characters would equal the voice daemon's (omarchyvr-voice)
    // SAFETY: PR_SET_NAME reads a NUL-terminated string of at most 16 bytes.
    unsafe {
        libc::prctl(libc::PR_SET_NAME, c"omarchyvr-vhud".as_ptr() as libc::c_ulong, 0, 0, 0);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Err(e) => {
            eprintln!("omarchyvr-voice-hud: {e}\n\n{}", cli::USAGE);
            ExitCode::from(2)
        }
        Ok(Command::Help) => {
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("omarchyvr-voice-hud {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Command::Preview(o)) => match preview::run(&o) {
            Ok(summary) => {
                println!("{summary}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                log::error(e);
                ExitCode::FAILURE
            }
        },
        Ok(Command::Dump(o)) => {
            sig::install();
            match dump::run(&o, &mut std::io::stdout(), &sig::STOP) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    log::error(e);
                    ExitCode::FAILURE
                }
            }
        }
        Ok(Command::Run(o)) => runner::run(o),
    }
}
