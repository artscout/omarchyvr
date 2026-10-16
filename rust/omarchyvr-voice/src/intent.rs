//! The intent layer: final transcripts -> named commands, for English and Spanish.
//!
//! Deliberately small: a rule has a name and phrase patterns; a pattern is literal words with an optional trailing
//! `{slot}` that captures the rest of the utterance. Matching works on normalised words (lower case, accents folded,
//! punctuation dropped, leading politeness like "please" / "por favor" / "okay" ignored), so "¡Recentra la vista!"
//! matches `recentra la vista`. `omarchyvr-voice` only *reports* intents (an `intent` event on the socket); acting on them
//! (with confirmation for anything risky) is the subscriber's job. Rules can be extended from a JSON file:
//! `[{"name": "app.open", "patterns": ["open {app}", "abre {app}"]}]`.

use serde::Deserialize;
use std::collections::BTreeMap;

/// A matched intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intent {
    pub name: String,
    pub slots: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    words: Vec<String>,
    slot: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    name: String,
    patterns: Vec<Pattern>,
}

/// The rule set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Intents {
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct RuleJson {
    name: String,
    patterns: Vec<String>,
}

/// Leading words that do not change the meaning.
const FILLERS: &[&str] = &[
    "please",
    "por",
    "favor",
    "okay",
    "ok",
    "hey",
    "oye",
    "vale",
    "computer",
    "computadora",
];

fn fold(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        c => c,
    }
}

/// Lower case, accents folded, everything but letters/digits/apostrophes as separators.
pub fn normalize(text: &str) -> Vec<String> {
    let s: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(fold)
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect();
    s.split_whitespace().map(str::to_string).collect()
}

fn parse_pattern(p: &str) -> Result<Pattern, String> {
    let mut words = Vec::new();
    let mut slot = None;
    let parts: Vec<&str> = p.split_whitespace().collect();
    for (i, part) in parts.iter().enumerate() {
        if let Some(name) = part.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            if i + 1 != parts.len() || name.is_empty() {
                return Err(format!("pattern {p:?}: a {{slot}} must be the last word"));
            }
            slot = Some(name.to_string());
        } else {
            words.extend(normalize(part));
        }
    }
    if words.is_empty() {
        return Err(format!("pattern {p:?} has no words"));
    }
    Ok(Pattern { words, slot })
}

impl Intents {
    /// No rules.
    pub fn none() -> Intents {
        Intents::default()
    }

    /// The built-in rules: only what the debug HUD and the view need today.
    pub fn builtin() -> Intents {
        let rules: &[(&str, &[&str])] = &[
            (
                "hud.clear",
                &[
                    "clear",
                    "clear screen",
                    "clear the screen",
                    "clear text",
                    "clear the text",
                    "borra",
                    "borrar",
                    "borra la pantalla",
                    "borra el texto",
                    "limpia",
                    "limpiar",
                    "limpia la pantalla",
                ],
            ),
            (
                "vr.recenter",
                &[
                    "recenter",
                    "recenter view",
                    "recenter the view",
                    "recentra",
                    "recentrar",
                    "recentra la vista",
                    "centra la vista",
                ],
            ),
        ];
        let mut out = Intents::default();
        for (name, pats) in rules {
            out.add(name, pats).expect("built-in patterns are valid");
        }
        out
    }

    /// Add a rule (or more patterns to an existing one).
    pub fn add(&mut self, name: &str, patterns: &[&str]) -> Result<(), String> {
        let pats = patterns
            .iter()
            .map(|p| parse_pattern(p))
            .collect::<Result<Vec<_>, _>>()?;
        match self.rules.iter_mut().find(|r| r.name == name) {
            Some(r) => r.patterns.extend(pats),
            None => self.rules.push(Rule {
                name: name.to_string(),
                patterns: pats,
            }),
        }
        Ok(())
    }

    /// Add the rules of a JSON file's text (see the module docs).
    pub fn extend_from_json(&mut self, json: &str) -> Result<(), String> {
        let rules: Vec<RuleJson> =
            serde_json::from_str(json).map_err(|e| format!("intent rules: {e}"))?;
        for r in rules {
            let pats: Vec<&str> = r.patterns.iter().map(String::as_str).collect();
            self.add(&r.name, &pats)?;
        }
        Ok(())
    }

    /// Number of rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The first rule (in insertion order) that matches the whole utterance.
    pub fn match_text(&self, text: &str) -> Option<Intent> {
        let words = normalize(text);
        let skip = words
            .iter()
            .take_while(|w| FILLERS.contains(&w.as_str()))
            .count();
        let words = &words[skip..];
        if words.is_empty() {
            return None;
        }
        for r in &self.rules {
            for p in &r.patterns {
                if words.len() < p.words.len() || words[..p.words.len()] != p.words[..] {
                    continue;
                }
                let rest = &words[p.words.len()..];
                match &p.slot {
                    None if rest.is_empty() => {
                        return Some(Intent {
                            name: r.name.clone(),
                            slots: BTreeMap::new(),
                        })
                    }
                    Some(slot) if !rest.is_empty() => {
                        let mut slots = BTreeMap::new();
                        slots.insert(slot.clone(), rest.join(" "));
                        return Some(Intent {
                            name: r.name.clone(),
                            slots,
                        });
                    }
                    _ => {}
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation() {
        assert_eq!(
            normalize("¡Recentra la Vista!"),
            vec!["recentra", "la", "vista"]
        );
        assert_eq!(
            normalize("Open the browser."),
            vec!["open", "the", "browser"]
        );
        assert_eq!(normalize("¿Qué tal, señor?"), vec!["que", "tal", "senor"]);
        assert_eq!(normalize("  ...  "), Vec::<String>::new());
    }

    #[test]
    fn builtin_rules_in_both_languages() {
        let i = Intents::builtin();
        let name = |t: &str| i.match_text(t).map(|x| x.name);
        assert_eq!(name("Clear the screen."), Some("hud.clear".into()));
        assert_eq!(name("Borra la pantalla"), Some("hud.clear".into()));
        assert_eq!(
            name("Please recenter the view."),
            Some("vr.recenter".into())
        );
        assert_eq!(
            name("Por favor, recentra la vista."),
            Some("vr.recenter".into())
        );
        assert_eq!(name("¡Recentra!"), Some("vr.recenter".into()));
        // whole utterance only: dictation that merely contains the words is not a command
        assert_eq!(name("I want to clear the screen later"), None);
        assert_eq!(name("clear the screen and the desk"), None);
        assert_eq!(name("please"), None);
        assert_eq!(name(""), None);
    }

    #[test]
    fn slots_and_json_rules() {
        let mut i = Intents::none();
        assert!(i.is_empty());
        i.extend_from_json(r#"[{"name":"app.open","patterns":["open {app}","abre {app}"]},{"name":"x","patterns":["stop"]}]"#)
            .unwrap();
        assert_eq!(i.len(), 2);
        let m = i.match_text("Abre el navegador.").unwrap();
        assert_eq!(m.name, "app.open");
        assert_eq!(m.slots.get("app").map(String::as_str), Some("el navegador"));
        // a slot needs at least one word
        assert_eq!(i.match_text("open"), None);
        assert!(i
            .extend_from_json(r#"[{"name":"bad","patterns":["{x} first"]}]"#)
            .is_err());
        assert!(i
            .extend_from_json(r#"[{"name":"bad","patterns":["..."]}]"#)
            .is_err());
        assert!(i.extend_from_json("not json").is_err());
        // more patterns for an existing rule
        i.add("x", &["detente"]).unwrap();
        assert_eq!(i.len(), 2);
        assert_eq!(i.match_text("Detente.").map(|m| m.name), Some("x".into()));
    }
}
