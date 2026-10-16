//! EDID parsing for finding the headset's DRM connector: the Dream Air's EDID (`linux/pimax-dp-edid*.bin`) has the
//! manufacturer id `PVR` and product code 0x1019.

/// EDID manufacturer id `PVR` as the little-endian u16 of header bytes 8..10 (the bytes are `42 d2`).
pub const PIMAX_VENDOR: u16 = 0xD242;
/// EDID product code (bytes 10..12, little-endian).
pub const PIMAX_PRODUCT: u16 = 0x1019;

/// Vendor and product of an EDID base block, `None` if it is too short or lacks the fixed header.
pub fn edid_ids(edid: &[u8]) -> Option<(u16, u16)> {
    const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];
    if edid.len() < 12 || edid[..8] != HEADER {
        return None;
    }
    Some((
        u16::from_le_bytes([edid[8], edid[9]]),
        u16::from_le_bytes([edid[10], edid[11]]),
    ))
}

/// The three-letter PNP manufacturer id of the vendor field (`0xD242` is "PVR").
pub fn pnp_id(vendor_le: u16) -> String {
    let v = u16::from_be_bytes(vendor_le.to_le_bytes());
    (0..3)
        .map(|i| {
            let c = ((v >> (10 - 5 * i)) & 0x1f) as u8;
            if (1..=26).contains(&c) {
                (b'A' + c - 1) as char
            } else {
                '?'
            }
        })
        .collect()
}

/// True if this EDID belongs to the Pimax Dream Air.
pub fn is_headset(edid: &[u8]) -> bool {
    edid_ids(edid) == Some((PIMAX_VENDOR, PIMAX_PRODUCT))
}

/// `card1-DP-1` to `DP-1`.
pub fn connector_of_entry(entry: &str) -> Option<&str> {
    let rest = entry.strip_prefix("card")?;
    let (idx, name) = rest.split_once('-')?;
    (!idx.is_empty() && idx.bytes().all(|b| b.is_ascii_digit()) && !name.is_empty()).then_some(name)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn headset_edid() -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
        e[8] = 0x42;
        e[9] = 0xd2;
        e[10] = 0x19;
        e[11] = 0x10;
        e
    }

    #[test]
    fn recognises_the_dream_air() {
        let e = headset_edid();
        assert_eq!(edid_ids(&e), Some((0xD242, 0x1019)));
        assert!(is_headset(&e));
        assert_eq!(pnp_id(0xD242), "PVR");
    }

    #[test]
    fn other_monitors_and_garbage_are_not_the_headset() {
        let mut dell = headset_edid();
        dell[8] = 0x10; // DEL
        dell[9] = 0xac;
        assert!(!is_headset(&dell));
        assert_eq!(pnp_id(0xac10), "DEL");
        let mut other_product = headset_edid();
        other_product[10] = 0x01;
        assert!(!is_headset(&other_product));
        assert!(!is_headset(&[]));
        assert!(!is_headset(&[0u8; 128])); // no EDID header
        assert!(edid_ids(&headset_edid()[..11]).is_none());
    }

    #[test]
    fn connector_names() {
        assert_eq!(connector_of_entry("card1-DP-1"), Some("DP-1"));
        assert_eq!(connector_of_entry("card0-HDMI-A-2"), Some("HDMI-A-2"));
        assert_eq!(connector_of_entry("card1"), None);
        assert_eq!(connector_of_entry("renderD128"), None);
        assert_eq!(connector_of_entry("cardx-DP-1"), None);
    }

    #[test]
    fn real_edid_files_of_the_repo_match() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../linux");
        let mut n = 0;
        for f in std::fs::read_dir(dir).unwrap().flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if name.starts_with("pimax-dp-edid") && name.ends_with(".bin") {
                assert!(is_headset(&std::fs::read(f.path()).unwrap()), "{name}");
                n += 1;
            }
        }
        assert!(n >= 1);
    }
}
