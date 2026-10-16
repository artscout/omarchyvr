fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(pimax_components::health::main(&a));
}
