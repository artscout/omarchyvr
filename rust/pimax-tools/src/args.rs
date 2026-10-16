//! Tiny argument helper (no clap: keeps the dependency tree minimal).
use crate::CliError;

/// Remaining arguments of a subcommand.
pub struct Args(pub Vec<String>);

impl Args {
    /// Remove and report a boolean flag.
    pub fn flag(&mut self, name: &str) -> bool {
        match self.0.iter().position(|a| a == name) {
            Some(i) => {
                self.0.remove(i);
                true
            }
            None => false,
        }
    }

    /// Remove `--name VALUE` or `--name=VALUE`.
    pub fn opt(&mut self, name: &str) -> Result<Option<String>, CliError> {
        if let Some(i) = self.0.iter().position(|a| a == name) {
            if i + 1 >= self.0.len() {
                return Err(CliError::usage(format!("{name} needs a value")));
            }
            let v = self.0.remove(i + 1);
            self.0.remove(i);
            return Ok(Some(v));
        }
        let pre = format!("{name}=");
        if let Some(i) = self.0.iter().position(|a| a.starts_with(&pre)) {
            return Ok(Some(self.0.remove(i)[pre.len()..].to_string()));
        }
        Ok(None)
    }

    /// Remove and parse an option.
    pub fn opt_parse<T: std::str::FromStr>(&mut self, name: &str) -> Result<Option<T>, CliError> {
        match self.opt(name)? {
            Some(v) => v
                .parse()
                .map(Some)
                .map_err(|_| CliError::usage(format!("bad value for {name}: {v}"))),
            None => Ok(None),
        }
    }

    /// Error out on leftovers that look like options.
    pub fn no_unknown_flags(&self) -> Result<(), CliError> {
        match self.0.iter().find(|a| a.starts_with("--")) {
            Some(a) => Err(CliError::usage(format!("unknown option {a}"))),
            None => Ok(()),
        }
    }

    /// Positional `i` parsed.
    pub fn pos<T: std::str::FromStr>(&self, i: usize) -> Result<Option<T>, CliError> {
        match self.0.iter().filter(|a| !a.starts_with("--")).nth(i) {
            Some(v) => v
                .parse()
                .map(Some)
                .map_err(|_| CliError::usage(format!("bad argument: {v}"))),
            None => Ok(None),
        }
    }
}
