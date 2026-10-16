//! Per-headset profiles for the Pimax Dream Air (design: `analysis/research/portability-and-calibration-gui.md` 8.3, 8.4).
//!
//! ```text
//! <root>/                      (default ~/.config/omarchyvr)
//!   active -> profiles/<serial>/<name>
//!   lens.conf                  (legacy path; optionally a symlink to active/lens.conf)
//!   profiles/<serial>/<name>/  lens.conf setup.toml meta.toml eye-correction.toml cam-calib-overlay.json passthrough.toml
//!   backups/<serial>/<stamp>-<nn>-<reason>/  full copy of the profile before every write (last 30 kept)
//! <cache>/<serial>/gyro-bias   (default ~/.cache/omarchyvr)
//! ```
//! Rules enforced here: every write is atomic (temp file, fsync, rename) and preceded by a backup; lens.conf is validated
//! with the `pimax-lens` parser before it is written; serials and names are path-safe; a profile of another serial is
//! never activated without an explicit confirmation. Nothing here talks to the headset.
//!
//! [`ai`]: the AI providers (`~/.config/omarchyvr/ai.toml`, private) shared by the app and voice control.
pub mod ai;
pub mod claude_cli;
pub mod ctrl_inhand;
pub mod diff;
pub mod eye;
pub mod migrate;
pub mod paths;
pub mod setup;
pub mod store;

pub use migrate::{migrate_home, migrate_legacy_dirs};
pub use diff::{diff_lines, DiffLine};
pub use setup::Setup;
pub use store::{
    check_serial, gyro_bias_path, legacy_gyro_bias_path, BackupInfo, ConfigError, ProfileId,
    SerialCheck, Store, MANAGED_FILES,
};
