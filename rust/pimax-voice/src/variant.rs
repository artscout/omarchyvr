//! Experiment variants for the state audit (`pimax-voice audit --state --variant NAME|FILE`): one small TOML says
//! which model, router, prompt, state blocks and tool scoping a run uses, so a change can be measured against the
//! same cases (docs/VOICE-STATE-TESTS.md). Built-in variants live in `tests/state/variants/*.toml`.
//!
//! The runner applies what the pipeline already exposes (model endpoint and alias, Lara on/off and its URL, the goal
//! model). The planner reads the rest through [`active`] and says which knobs it used with [`honour`]; the report
//! lists every knob a variant set that no code honoured, so a run never claims to test something it did not.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Model {
    /// `.../v1/chat/completions`; empty = ai.toml's.
    pub endpoint: String,
    /// The model alias the server knows ("pimax-local"); empty = ai.toml's.
    pub alias: String,
    pub temperature: f32,
    /// The GBNF grammar on the answer.
    pub grammar: bool,
    pub thinking: bool,
}

impl Default for Model {
    fn default() -> Self {
        Model { endpoint: String::new(), alias: String::new(), temperature: 0.0, grammar: true, thinking: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Router {
    /// on | off
    pub lara: String,
    /// Empty = ai.toml's.
    pub url: String,
    /// A label for the report (which tuned Lara).
    pub version: String,
}

impl Default for Router {
    fn default() -> Self {
        Router { lara: "on".into(), url: String::new(), version: "lara".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prompt {
    /// current | a path to a template file
    pub template: String,
    /// The state blocks given to the model ([`crate::world::describe`]): screens, apps, focus, gaze, tabs, session,
    /// view, media; plus hints (the fuzzy matcher's candidates).
    pub blocks: Vec<String>,
    /// A "target:" line (the state the user wants) the model plans towards.
    pub target_line: bool,
}

impl Default for Prompt {
    fn default() -> Self {
        Prompt { template: "current".into(), blocks: Vec::new(), target_line: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tools {
    /// all | category | category+hints | topk
    pub scope: String,
    pub top_k: usize,
    /// The scoped planner falls back to the single prompt with every tool when its answer fails or is none.
    pub fallback: bool,
}

impl Default for Tools {
    fn default() -> Self {
        Tools { scope: "all".into(), top_k: 12, fallback: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GoalModel {
    pub endpoint: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Run {
    /// 0 = every case of the split.
    pub subset: usize,
    pub seed: u64,
    /// dev | held | all
    pub split: String,
}

impl Default for Run {
    fn default() -> Self {
        Run { subset: 0, seed: 1, split: "all".into() }
    }
}

/// One experiment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Variant {
    pub name: String,
    pub notes: String,
    pub model: Model,
    pub goal_model: GoalModel,
    pub router: Router,
    pub prompt: Prompt,
    pub tools: Tools,
    pub run: Run,
}

impl Default for Variant {
    fn default() -> Self {
        Variant { name: "baseline".into(), notes: String::new(), model: Model::default(), goal_model: GoalModel::default(), router: Router::default(), prompt: Prompt::default(), tools: Tools::default(), run: Run::default() }
    }
}

/// The built-in variants (`tests/state/variants/*.toml`).
pub const BUILTIN: &[(&str, &str)] = &[
    ("baseline", include_str!("../tests/state/variants/baseline.toml")),
    ("lara-off", include_str!("../tests/state/variants/lara-off.toml")),
    ("state-prompt", include_str!("../tests/state/variants/state-prompt.toml")),
    ("qwen35-0.8b", include_str!("../tests/state/variants/qwen35-0.8b.toml")),
    ("qwen35-2b", include_str!("../tests/state/variants/qwen35-2b.toml")),
    ("qwen35-4b", include_str!("../tests/state/variants/qwen35-4b.toml")),
    ("qwen35-9b", include_str!("../tests/state/variants/qwen35-9b.toml")),
    ("qwen36-27b", include_str!("../tests/state/variants/qwen36-27b.toml")),
];

impl Variant {
    pub fn parse(text: &str) -> Result<Variant, String> {
        toml::from_str(text).map_err(|e| format!("variant: {e}"))
    }

    /// A built-in name or a TOML file.
    pub fn load(name_or_path: &str) -> Result<Variant, String> {
        if let Some((_, t)) = BUILTIN.iter().find(|(n, _)| *n == name_or_path) {
            return Variant::parse(t);
        }
        let t = std::fs::read_to_string(name_or_path).map_err(|e| format!("{name_or_path}: {e} (built-in: {})", BUILTIN.iter().map(|b| b.0).collect::<Vec<_>>().join(", ")))?;
        Variant::parse(&t)
    }

    /// The knobs this variant sets away from the defaults ("model.temperature", "prompt.blocks", ...).
    pub fn set_knobs(&self) -> Vec<String> {
        let d = Variant::default();
        let mut v = Vec::new();
        let mut k = |c: bool, n: &str| {
            if c {
                v.push(n.to_string());
            }
        };
        k(!self.model.endpoint.is_empty(), "model.endpoint");
        k(!self.model.alias.is_empty(), "model.alias");
        k(self.model.temperature != d.model.temperature, "model.temperature");
        k(self.model.grammar != d.model.grammar, "model.grammar");
        k(self.model.thinking != d.model.thinking, "model.thinking");
        k(!self.goal_model.endpoint.is_empty(), "goal_model.endpoint");
        k(self.router.lara != d.router.lara, "router.lara");
        k(!self.router.url.is_empty(), "router.url");
        k(self.prompt.template != d.prompt.template, "prompt.template");
        k(!self.prompt.blocks.is_empty(), "prompt.blocks");
        k(self.prompt.target_line, "prompt.target_line");
        k(self.tools.scope != d.tools.scope, "tools.scope");
        k(self.tools.fallback != d.tools.fallback, "tools.fallback");
        v
    }
}

static ACTIVE: OnceLock<Variant> = OnceLock::new();
static HONOURED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Set the variant of this run (once; the state audit does it before the first case).
pub fn set_active(v: Variant) {
    let _ = ACTIVE.set(v);
}

/// The variant of this run, None outside an experiment (the pipeline then uses ai.toml as always).
pub fn active() -> Option<&'static Variant> {
    ACTIVE.get()
}

/// Code that reads a knob of the active variant says so ("prompt.blocks"); the report lists the ones nobody read.
pub fn honour(knob: &str) {
    if let Ok(mut h) = HONOURED.lock() {
        h.insert(knob.to_string());
    }
}

pub fn honoured() -> BTreeSet<String> {
    HONOURED.lock().map(|h| h.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_variants_parse() {
        for (n, t) in BUILTIN {
            let v = Variant::parse(t).unwrap();
            assert_eq!(&v.name, n);
        }
        assert!(Variant::load("lara-off").unwrap().set_knobs().contains(&"router.lara".to_string()));
        assert!(Variant::load("baseline").unwrap().set_knobs().is_empty());
    }
}
