#![allow(dead_code)]
use std::io::Read;
use std::path::PathBuf;

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

pub fn load_rec(name: &str) -> Vec<(f64, [u8; 64])> {
    let f = std::fs::File::open(fixtures_dir().join(name)).unwrap();
    let mut d = Vec::new();
    flate2::read::GzDecoder::new(f).read_to_end(&mut d).unwrap();
    d.as_chunks::<72>()
        .0
        .iter()
        .map(|c| {
            let mut b = [0u8; 64];
            b.copy_from_slice(&c[8..]);
            (f64::from_le_bytes(c[..8].try_into().unwrap()), b)
        })
        .collect()
}

pub fn tmpdir(name: &str) -> PathBuf {
    let d =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
