//! Headset models that speak this protocol, told apart by the USB product id of the HID chip.
//!
//! The Crystal Super answers the same `58 ..` frames on the same four interfaces as the Dream Air
//! (analysis/crystal-super-capture-20261017.md); only the ids, the default video rate and the optics differ.

/// One headset model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    /// Device module id (`rust/pimax-device/devices/<id>`).
    pub id: &'static str,
    /// Name shown to the user.
    pub name: &'static str,
    /// Product id of the HID chip (IF0..IF3).
    pub pid: u16,
    /// Product id of the display bridge MCU (ASCII log).
    pub pid_bridge: u16,
    /// Video rate Pimax Play announces at a cold start (`58 0d 1a 05 02 FPS`, keepalive rate byte).
    pub default_fps: u8,
}

/// Pimax Dream Air (analysis/init-sequence.md).
pub const DREAM_AIR: Model = Model { id: "dream-air", name: "Pimax Dream Air", pid: 0x0044, pid_bridge: 0x0043, default_fps: 72 };

/// Pimax Crystal Super (Windows capture 2026-10-17, take cold2: keepalive `58 04 39 00 5a 00`).
pub const CRYSTAL_SUPER: Model = Model { id: "crystal-super", name: "Pimax Crystal Super", pid: 0x0040, pid_bridge: 0x0041, default_fps: 90 };

/// Every supported model, the Dream Air first.
pub const MODELS: &[Model] = &[DREAM_AIR, CRYSTAL_SUPER];

/// The model with this HID-chip product id.
pub fn by_pid(pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.pid == pid)
}

/// The model whose display bridge has this product id.
pub fn by_bridge_pid(pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.pid_bridge == pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_found() {
        for m in MODELS {
            assert_eq!(by_pid(m.pid), Some(m));
            assert_eq!(by_bridge_pid(m.pid_bridge), Some(m));
            assert!(by_pid(m.pid_bridge).is_none(), "{} bridge id is not a headset", m.id);
        }
        assert_eq!(by_pid(0x0044).unwrap().id, "dream-air");
        assert_eq!(by_pid(0x0040).unwrap().id, "crystal-super");
        assert!(by_pid(0x0042).is_none());
    }
}
