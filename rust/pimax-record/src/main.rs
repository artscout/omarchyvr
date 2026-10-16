fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(pimax_record::cli::main(&args));
}
