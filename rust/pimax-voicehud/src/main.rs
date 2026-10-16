use pimax_voicehud::cli::{self, Command};
use pimax_voicehud::{dump, log, preview, runner, sig};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Err(e) => {
            eprintln!("pimax-voice-hud: {e}\n\n{}", cli::USAGE);
            ExitCode::from(2)
        }
        Ok(Command::Help) => {
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("pimax-voice-hud {}", env!("CARGO_PKG_VERSION"));
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
