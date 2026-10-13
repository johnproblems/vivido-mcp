//! Safety tiers for Vivido automation tools.
//!
//! Three cumulative tiers gate what a connected model may do:
//!
//! - `observe` (always on): read state, wait for state, capture screenshots.
//! - `input` (`--allow input`): type, press keys, paste, move the mouse, drop files.
//! - `process` (`--allow process`): everything else that mutates — windows, config,
//!   signals, quit, headless lifecycle, and any unknown or host-claimed mutating method.
//!
//! The mapping is derived from the live `hello` handshake's `method_capabilities`, never
//!! from a hand-kept list, so the server tracks Vivido versions without edits. One rule is
//! absolute: a method `hello` marks `mutating` is never placed in `observe`, whatever its
//! class claims.

use std::fmt::Write as _;

/// One of the three cumulative safety tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Read-only observation: state reads, waits, screenshots, event drains.
    Observe,
    /// Terminal input injection: typing, keys, paste, mouse, file drops.
    Input,
    /// Process and workspace mutation: windows, config, signals, quit, headless.
    Process,
}

impl Tier {
    /// The `--allow` spelling that unlocks this tier.
    pub fn allow_name(self) -> &'static str {
        match self {
            Tier::Observe => "observe",
            Tier::Input => "input",
            Tier::Process => "process",
        }
    }
}

/// Which tiers the operator enabled at launch. `observe` is always on.
#[derive(Clone, Copy, Debug, Default)]
pub struct Allowance {
    /// `--allow input` (or the broader `--allow process`) was given.
    pub input: bool,
    /// `--allow process` was given.
    pub process: bool,
}

impl Allowance {
    /// Whether `tier` is usable under this allowance. Tiers are cumulative.
    pub fn allows(self, tier: Tier) -> bool {
        match tier {
            Tier::Observe => true,
            Tier::Input => self.input || self.process,
            Tier::Process => self.process,
        }
    }
}

/// Parse `--allow` values. Accepts repeats and comma-separated lists, case-insensitively.
/// Tiers are cumulative: `process` implies `input`.
pub fn parse_allow(values: &[String]) -> Result<Allowance, String> {
    let mut allowance = Allowance::default();
    for value in values {
        for part in value.split(',') {
            match part.trim().to_ascii_lowercase().as_str() {
                "" => {}
                "observe" => {}
                "input" => allowance.input = true,
                "process" | "all" => {
                    allowance.input = true;
                    allowance.process = true;
                }
                other => {
                    return Err(format!(
                        "unknown tier {other:?}: --allow accepts observe, input, process, all"
                    ));
                }
            }
        }
    }
    Ok(allowance)
}

/// Classify one advertised method into a tier.
///
/// `class` and `mutating` come straight from the `hello` handshake. The mutating flag
/// wins over the class: anything that changes host state needs at least `input`, and
/// only the `input` class lands there — every other mutating method needs `process`.
pub fn tier_for_method(class: &str, mutating: bool) -> Tier {
    if !mutating && class == "observe" {
        return Tier::Observe;
    }
    if mutating && class == "input" {
        return Tier::Input;
    }
    // Everything else lands in `process`: every mutating method outside `input`, and —
    // conservatively — any non-mutating method outside `observe`. An unfamiliar class
    // is not a read-only promise, so it does not earn the default tier.
    Tier::Process
}

/// Refusal message for a tool call above the granted tier. Always names `--allow`
/// and the allowance the server actually started with.
pub fn refusal(tool: &str, tier: Tier, allowance: Allowance) -> String {
    let started = if allowance.process {
        "--allow process"
    } else if allowance.input {
        "--allow input"
    } else {
        "observe-only"
    };
    let mut message = format!(
        "tool `{tool}` needs tier `{}` but vivido-mcp was started {started}; \
         restart it with `--allow {}`",
        tier.allow_name(),
        tier.allow_name()
    );
    if tier == Tier::Input {
        let _ = write!(
            message,
            " (or the broader `--allow process`). Prefer an ephemeral headless session \
             for agent input work so typing cannot reach your visible workspace."
        );
    } else {
        let _ = write!(
            message,
            ". This tier can create and close windows, signal processes, and stop \
             instances; prefer an ephemeral headless session for agent work."
        );
    }
    message
}

/// MCP tool annotations mirroring the tier, so client approval UIs prompt correctly.
pub fn annotations(tier: Tier) -> serde_json::Value {
    match tier {
        Tier::Observe => {
            serde_json::json!({
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false,
            })
        }
        Tier::Input => {
            serde_json::json!({
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": false,
            })
        }
        Tier::Process => {
            serde_json::json!({
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": false,
                "openWorldHint": false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_are_cumulative() {
        let none = Allowance::default();
        assert!(none.allows(Tier::Observe));
        assert!(!none.allows(Tier::Input));
        assert!(!none.allows(Tier::Process));

        let input = parse_allow(&["input".to_owned()]).unwrap();
        assert!(input.allows(Tier::Observe));
        assert!(input.allows(Tier::Input));
        assert!(!input.allows(Tier::Process));

        let process = parse_allow(&["process".to_owned()]).unwrap();
        assert!(process.allows(Tier::Observe));
        assert!(process.allows(Tier::Input));
        assert!(process.allows(Tier::Process));
    }

    #[test]
    fn allow_accepts_repeats_and_commas() {
        let parsed = parse_allow(&["Input".to_owned(), "observe,process".to_owned()]).unwrap();
        assert!(parsed.allows(Tier::Process));
        assert!(parse_allow(&["everything".to_owned()]).is_err());
    }

    #[test]
    fn all_is_an_alias_for_process() {
        let parsed = parse_allow(&["all".to_owned()]).unwrap();
        assert!(parsed.allows(Tier::Input));
        assert!(parsed.allows(Tier::Process));
    }

    #[test]
    fn mutating_never_lands_in_observe() {
        for class in
            ["observe", "input", "window", "config", "process", "lifecycle", "extension", ""]
        {
            assert_ne!(
                tier_for_method(class, true),
                Tier::Observe,
                "mutating {class} must not be observe"
            );
        }
        assert_eq!(tier_for_method("observe", false), Tier::Observe);
        assert_eq!(tier_for_method("input", true), Tier::Input);
        assert_eq!(tier_for_method("window", true), Tier::Process);
        assert_eq!(tier_for_method("extension", true), Tier::Process);
    }

    #[test]
    fn refusal_names_allow() {
        for tier in [Tier::Input, Tier::Process] {
            let message = refusal("key", tier, Allowance::default());
            assert!(message.contains("--allow"), "{message}");
            assert!(message.contains(tier.allow_name()), "{message}");
            assert!(message.contains("observe-only"), "{message}");
        }
        let message = refusal("quit", Tier::Process, parse_allow(&["input".to_owned()]).unwrap());
        assert!(message.contains("--allow input"), "{message}");
        assert!(message.contains("--allow process"), "{message}");
    }
}
