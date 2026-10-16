fn main() {
    omarchyvr_config::migrate_home(); // one time: ~/.config/omarchyvr etc. -> omarchyvr
    omarchyvr_autod::cli::main();
}
