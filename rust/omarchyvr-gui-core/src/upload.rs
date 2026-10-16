//! Sending crash reports somewhere: designed, not built. The app has no network code; a problem report is a file the
//! user looks at and shares by hand.
//!
//! The design, for when a receiving service exists (docs/GUI.md, "Crash reports"):
//! * opt-in only: off by default, a switch in the app, and every upload is a separate click that shows the exact
//!   file first; never in the background;
//! * only the redacted bundle of [`crate::bundle`] (minidumps only with their own extra opt-in: they hold memory);
//! * HTTPS POST to a configured endpoint, no endpoint compiled in, no account, no identifier beyond the report's.
//!
//! [`UploadSettings::check`] always refuses today.

/// Upload settings (all off).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UploadSettings {
    /// The user switched uploads on.
    pub enabled: bool,
    /// Where reports would go (none is configured anywhere).
    pub endpoint: Option<String>,
    /// Also send minidumps.
    pub include_minidumps: bool,
}

impl UploadSettings {
    /// May a report be sent? Never in this version.
    pub fn check(&self) -> Result<(), &'static str> {
        if !self.enabled {
            return Err("uploads are off");
        }
        if self.endpoint.is_none() {
            return Err("no upload endpoint is configured");
        }
        Err("uploading is not implemented in this version: share the report file by hand")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_ever_sent() {
        let mut s = UploadSettings::default();
        assert!(!s.enabled && s.endpoint.is_none());
        assert!(s.check().is_err());
        s.enabled = true;
        s.endpoint = Some("https://example.invalid".into());
        assert!(s.check().unwrap_err().contains("not implemented"));
    }
}
