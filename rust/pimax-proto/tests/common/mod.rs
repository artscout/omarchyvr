#![allow(dead_code)]
//! Fixture loading shared by the integration tests (real captures, see rust/fixtures/make_fixtures.py).
use std::io::Read;
use std::path::PathBuf;

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

pub fn gunzip(name: &str) -> Vec<u8> {
    let f = std::fs::File::open(fixtures_dir().join(name))
        .unwrap_or_else(|e| panic!("fixture {name}: {e}"));
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(f)
        .read_to_end(&mut out)
        .unwrap();
    out
}

/// `[f64 time][64 B]` records.
pub fn load_rec(name: &str) -> Vec<(f64, [u8; 64])> {
    let d = gunzip(name);
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

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
