//! Check results, text/JSON rendering and the fix plan.
use crate::json::Json;

/// Outcome of a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Informational only.
    Info,
    /// Fine.
    Ok,
    /// Works but may cause trouble, or could not be verified.
    Warn,
    /// VR will not work (or must not be started).
    Fail,
}

impl Level {
    /// Lower-case name used in JSON.
    pub fn name(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Fail => "fail",
        }
    }
}

/// How to fix a problem. Commands are only shown to the user, never run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// Why this is needed.
    pub why: String,
    /// Commands in order.
    pub commands: Vec<String>,
    /// Commands that undo it.
    pub undo: Vec<String>,
    /// Needs root.
    pub root: bool,
    /// A reboot is needed afterwards.
    pub reboot: bool,
}

impl Fix {
    /// A fix with the given explanation and commands.
    pub fn new(why: impl Into<String>, root: bool, commands: &[&str], undo: &[&str]) -> Fix {
        Fix {
            why: why.into(),
            commands: commands.iter().map(|s| s.to_string()).collect(),
            undo: undo.iter().map(|s| s.to_string()).collect(),
            root,
            reboot: false,
        }
    }

    /// Mark as needing a reboot.
    pub fn reboot(mut self) -> Fix {
        self.reboot = true;
        self
    }
}

/// One check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Stable id (`kernel-log`).
    pub id: &'static str,
    /// Human title.
    pub title: &'static str,
    /// Outcome.
    pub level: Level,
    /// What was found.
    pub detail: String,
    /// Fix, when not ok.
    pub fix: Option<Fix>,
}

/// All results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Results in check order.
    pub checks: Vec<Check>,
}

impl Report {
    /// Add a result.
    pub fn add(
        &mut self,
        id: &'static str,
        title: &'static str,
        level: Level,
        detail: impl Into<String>,
        fix: Option<Fix>,
    ) {
        self.checks.push(Check {
            id,
            title,
            level,
            detail: detail.into(),
            fix,
        });
    }

    /// Worst level (Info/Ok if nothing is wrong).
    pub fn worst(&self) -> Level {
        self.checks
            .iter()
            .map(|c| c.level)
            .max()
            .unwrap_or(Level::Ok)
    }

    /// Count of checks at a level.
    pub fn count(&self, l: Level) -> usize {
        self.checks.iter().filter(|c| c.level == l).count()
    }

    /// Result by id.
    pub fn get(&self, id: &str) -> Option<&Check> {
        self.checks.iter().find(|c| c.id == id)
    }

    /// True if any check failed.
    pub fn has_fail(&self) -> bool {
        self.worst() == Level::Fail
    }

    /// One-line summary.
    pub fn summary(&self) -> String {
        format!(
            "{} ok, {} warnings, {} failed",
            self.count(Level::Ok) + self.count(Level::Info),
            self.count(Level::Warn),
            self.count(Level::Fail)
        )
    }

    /// Plain text table.
    pub fn render(&self) -> String {
        let mut o = String::new();
        for c in &self.checks {
            let tag = match c.level {
                Level::Info => "info",
                Level::Ok => " ok ",
                Level::Warn => "WARN",
                Level::Fail => "FAIL",
            };
            o += &format!("[{tag}] {:<22} {}\n", c.title, c.detail);
        }
        o += &format!("\n{}\n", self.summary());
        o
    }

    /// JSON document.
    pub fn to_json(&self) -> Json {
        Json::Obj(vec![
            ("summary".into(), Json::s(self.summary())),
            ("worst".into(), Json::s(self.worst().name())),
            (
                "checks".into(),
                Json::Arr(
                    self.checks
                        .iter()
                        .map(|c| {
                            let mut m = vec![
                                ("id".to_string(), Json::s(c.id)),
                                ("title".into(), Json::s(c.title)),
                                ("level".into(), Json::s(c.level.name())),
                                ("detail".into(), Json::s(&c.detail)),
                            ];
                            m.push((
                                "fix".into(),
                                c.fix.as_ref().map_or(Json::Null, |f| {
                                    Json::Obj(vec![
                                        ("why".into(), Json::s(&f.why)),
                                        ("root".into(), Json::Bool(f.root)),
                                        ("reboot".into(), Json::Bool(f.reboot)),
                                        (
                                            "commands".into(),
                                            Json::Arr(f.commands.iter().map(Json::s).collect()),
                                        ),
                                        (
                                            "undo".into(),
                                            Json::Arr(f.undo.iter().map(Json::s).collect()),
                                        ),
                                    ])
                                }),
                            ));
                            Json::Obj(m)
                        })
                        .collect(),
                ),
            ),
        ])
    }

    /// The fix plan as text: commands to copy, never executed by this program.
    pub fn fix_plan(&self) -> String {
        let fixes: Vec<&Check> = self
            .checks
            .iter()
            .filter(|c| c.level >= Level::Warn && c.fix.is_some())
            .collect();
        if fixes.is_empty() {
            return "Nothing to fix.\n".into();
        }
        let mut o = String::from(
            "# Pimax fix plan. This program never runs these commands: review them and run them yourself.\n\
             # Lines starting with 'sudo' need root.\n",
        );
        for (i, c) in fixes.iter().enumerate() {
            let f = c.fix.as_ref().expect("filtered");
            o += &format!(
                "\n## {}. {} ({}): {}\n# why: {}\n",
                i + 1,
                c.title,
                c.level.name(),
                c.detail,
                f.why
            );
            for cmd in &f.commands {
                o += &format!("{cmd}\n");
            }
            if f.reboot {
                o += "# then reboot.\n";
            }
            if !f.undo.is_empty() {
                o += "# undo:\n";
                for u in &f.undo {
                    o += &format!("#   {u}\n");
                }
            }
        }
        o
    }
}
