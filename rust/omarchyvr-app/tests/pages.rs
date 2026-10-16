//! Every page draws from the demo data (idle and running desk session, the pkexec dialog, the wizard) without a
//! window, a GPU or any process being started, and the PNGs are not blank.
use std::path::PathBuf;

#[test]
fn every_page_renders_headlessly() {
    let dir: PathBuf = std::env::temp_dir().join(format!("omarchyvr-setup-pages-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let files = omarchyvr_app::shots::render_all(&dir).expect("render");
    assert!(files.len() >= 12, "{files:?}");
    for f in &files {
        let len = std::fs::metadata(f).unwrap().len();
        assert!(len > 20_000, "{} is only {len} bytes", f.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
}
