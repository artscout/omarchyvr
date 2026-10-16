//! `CMakeCache.txt` of a Monado build (LESSONS row 6: `PIMAX_CAM:BOOL=ON` is what decides whether the build has the
//! camera / 6DoF code; strings in the binary are not enough).
use std::collections::BTreeMap;

/// The entries of a cache file (`KEY:TYPE=VALUE`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CMakeCache {
    entries: BTreeMap<String, String>,
}

impl CMakeCache {
    /// Parse the text of `CMakeCache.txt` (comments and blank lines ignored).
    pub fn parse(text: &str) -> CMakeCache {
        let mut entries = BTreeMap::new();
        for l in text.lines() {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') || l.starts_with("//") {
                continue;
            }
            let Some((lhs, val)) = l.split_once('=') else {
                continue;
            };
            let key = lhs.split(':').next().unwrap_or(lhs).trim();
            if !key.is_empty() {
                entries.insert(key.to_string(), val.trim().to_string());
            }
        }
        CMakeCache { entries }
    }

    /// Raw value.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// CMake truth value (`ON`, `TRUE`, `1`, `YES`, `Y` and non-zero numbers are true).
    pub fn bool(&self, key: &str) -> Option<bool> {
        let v = self.get(key)?.trim().to_ascii_uppercase();
        Some(match v.as_str() {
            "ON" | "TRUE" | "YES" | "Y" => true,
            "OFF" | "FALSE" | "NO" | "N" | "" | "IGNORE" | "NOTFOUND" => false,
            other if other.ends_with("-NOTFOUND") => false,
            other => other.parse::<i64>().map(|n| n != 0).unwrap_or(false),
        })
    }

    /// `PIMAX_CAM` (None if the option is not in the cache at all: an old tree without the option).
    pub fn pimax_cam(&self) -> Option<bool> {
        self.bool("PIMAX_CAM")
    }

    /// Source directory of the build.
    pub fn source_dir(&self) -> Option<&str> {
        self.get("CMAKE_HOME_DIRECTORY")
            .or_else(|| self.get("Monado_SOURCE_DIR"))
    }

    /// `Ninja`, `Unix Makefiles`, ...
    pub fn generator(&self) -> Option<&str> {
        self.get("CMAKE_GENERATOR")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESK: &str = include_str!("../tests/fixtures/CMakeCache-desk.txt");
    const PIMAX: &str = include_str!("../tests/fixtures/CMakeCache-pimax.txt");

    #[test]
    fn desk_build_has_the_camera_code() {
        let c = CMakeCache::parse(DESK);
        assert_eq!(c.pimax_cam(), Some(true));
        assert_eq!(c.source_dir(), Some("/home/u/src/monado-desk"));
        assert_eq!(c.generator(), Some("Ninja"));
        assert_eq!(c.bool("PIMAX_USE_RUST_LIBPIMAX"), Some(true));
    }

    #[test]
    fn plain_build_is_off_and_missing_is_unknown() {
        assert_eq!(CMakeCache::parse(PIMAX).pimax_cam(), Some(false));
        assert_eq!(CMakeCache::parse("CMAKE_BUILD_TYPE:STRING=Release\n").pimax_cam(), None);
        let c = CMakeCache::parse("PIMAX_CAM:BOOL=1\nX:PATH=X-NOTFOUND\n// PIMAX_CAM:BOOL=OFF comment\n");
        assert_eq!(c.pimax_cam(), Some(true));
        assert_eq!(c.bool("X"), Some(false));
    }
}
