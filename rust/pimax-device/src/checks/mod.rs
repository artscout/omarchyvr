//! Example checks: two ported from omarchyvr-doctor (`setcap`, `merged-head-sync`), one new (`cpu.governor`), and a
//! bridge that runs all of omarchyvr-doctor's checks and maps them to findings until they are migrated one by one.

pub mod cpu;
pub mod doctor;
pub mod gpu;
pub mod runtime;

use crate::check::Check;

/// The default check list (order = display order).
pub fn default_checks() -> Vec<&'static dyn Check> {
    vec![&gpu::MergedHeadSync as &dyn Check, &cpu::CapSysNice, &cpu::Governor, &runtime::SteamVrGames]
}
