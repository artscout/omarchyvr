fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Unlocked handles: the live tracker's thread prints with `eprintln!`, which would wait forever for a stderr lock
    // held by this thread for the whole run (the old `stderr().lock()` here deadlocked `pimax-eye live` on the first
    // message from the tracker thread, leaving the chip claimed and undrained).
    let code = pimax_eye::cli::run(&args, &mut std::io::stdout(), &mut std::io::stderr());
    std::process::exit(code);
}
