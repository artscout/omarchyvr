//! One module per component (plan section 6.2). Lane A moves the launcher's start/stop code here (P3) as
//! `components/{broker,monado,wayvr,...}.rs`, each with a type implementing [`crate::Component`], and registers it
//! in [`registry`]. Until a module exists, [`Generic`] stands in: the table row, the shared read-only probe, and no
//! start/stop (omarchyvrd then falls back to the `omarchyvr` CLI or to respawning the captured argv).
pub mod broker;
pub mod ctrltrack;
pub mod monado;
pub mod passthrough;
pub mod perf;
pub mod proc;
pub mod record;
pub mod screens;
pub mod voice;
pub mod voicehud;
pub mod wayvr;

use crate::component::Component;
use crate::spec::{Spec, SPECS};

/// The table row alone.
pub struct Generic(pub &'static Spec);

impl Component for Generic {
    fn spec(&self) -> &'static Spec {
        self.0
    }
}

/// Every component, in table order. Lane A: replace `Generic` for an id by its module's type here.
pub fn registry() -> Vec<Box<dyn Component>> {
    SPECS
        .iter()
        .map(|s| -> Box<dyn Component> {
            match s.id {
                "broker" => Box::new(broker::Broker(s)),
                "monado" => Box::new(monado::Monado(s)),
                "wayvr" => Box::new(wayvr::Wayvr(s)),
                "passthrough" => Box::new(passthrough::Passthrough(s)),
                "ctrltrack" => Box::new(ctrltrack::Ctrltrack(s)),
                "perf" => Box::new(perf::Perf(s)),
                "screens" => Box::new(screens::Screens(s)),
                "voice.listen" => Box::new(voice::VoiceListen(s)),
                "voice.captions" => Box::new(voice::VoiceCaptions(s)),
                "voice.act" => Box::new(voice::VoiceAct(s)),
                "voicehud" => Box::new(voicehud::VoiceHud(s)),
                "record" => Box::new(record::Record(s)),
                _ => Box::new(Generic(s)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn registry_covers_the_table() {
        let r = super::registry();
        assert_eq!(r.len(), crate::spec::SPECS.len());
        assert!(r.iter().all(|c| crate::spec::spec(c.id()).is_some()));
    }
}
