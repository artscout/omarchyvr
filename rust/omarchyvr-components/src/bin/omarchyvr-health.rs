fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(omarchyvr_components::health::main(&a));
}
