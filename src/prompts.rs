//! MCP prompts: the agent workflow for no-shell clients.
//!
//! The PRD's open question — should the server expose the Agent Skill text as an
//! MCP prompt? — is answered yes. Shell users learn the discover → act → verify
//! loop from the bundled skill; MCP clients with no shell get the same workflows
//! here, phrased for these tools.

use serde_json::{Value, json};

/// One prompt definition.
struct Prompt {
    name: &'static str,
    description: &'static str,
    arguments: &'static [(&'static str, &'static str, bool)],
    text: fn(Option<&str>) -> String,
}

const PROMPTS: &[Prompt] = &[
    Prompt {
        name: "vivido_workflow",
        description: "Drive a Vivido terminal correctly: discover, read sequences before acting, \
            act with an explicit wait, verify with capture or grid.",
        arguments: &[("goal", "What you want to accomplish in the terminal.", false)],
        text: workflow_text,
    },
    Prompt {
        name: "vivido_headless_test",
        description: "Run a CLI or TUI test inside an ephemeral headless session and stop it \
            afterwards.",
        arguments: &[("goal", "What you want to test.", false)],
        text: headless_test_text,
    },
    Prompt {
        name: "vivido_debug",
        description: "Diagnose a misbehaving terminal session: snapshots, traces, event gaps, \
            and deterministic headless reproduction.",
        arguments: &[("symptom", "What is going wrong.", false)],
        text: debug_text,
    },
];

fn workflow_text(goal: Option<&str>) -> String {
    let goal = goal
        .filter(|text| !text.trim().is_empty())
        .map(|text| format!("Your goal: {text}\n\n"))
        .unwrap_or_default();
    format!(
        "{goal}You drive a Vivido terminal through these MCP tools. Work in a loop:\n\
        \n\
        1. DISCOVER. Call `list_windows`, then `inspect` for the window you will use. \
        Never predict window IDs across a structural change; rediscover after \
        `window_not_found` or a restart.\n\
        2. READ SEQUENCES BEFORE ACTING. Note `screen_sequence`, `frame_sequence`, and \
        `output_offset` from `inspect` so a later wait cannot be satisfied by state you \
        already saw.\n\
        3. ACT WITH AN EXPLICIT WAIT. Type with `typing` (literal text, no Enter) or \
        `paste`, press keys with `key` (the only correct way to send Enter, Ctrl-C, \
        arrows, function keys), click with `mouse`. Then wait for what you actually \
        expect: `wait_text` for visible text, `wait_output` for bytes that may scroll \
        past, `wait_screen_stable` for a TUI settling, `wait_frame` for rendering, \
        `wait_exit` for a process, `wait_prompt` / `wait_command_finish` for shell \
        state. Never sleep.\n\
        4. VERIFY. Call `capture` (it settles and returns the screenshot as an image) \
        and look at it, or `get_grid` when the question is about content — positioned \
        cells beat plain text for menus, columns, and disabled controls. `get_text` is \
        for quick text; `transcript` for output that scrolled past.\n\
        \n\
        Prefer an ephemeral headless session (`headless_start`) for agent work so input \
        cannot reach the operator's visible workspace, and `headless_stop` it when done. \
        Keep control local and never emit endpoint paths or tokens."
    )
}

fn headless_test_text(goal: Option<&str>) -> String {
    let goal = goal
        .filter(|text| !text.trim().is_empty())
        .map(|text| format!("Your goal: {text}\n\n"))
        .unwrap_or_default();
    format!(
        "{goal}Test a CLI or TUI inside a disposable headless Vivido session:\n\
        \n\
        1. `headless_start` with a `size` matching the test (use WIDTHxHEIGHTpx when \
        screenshots matter). Note the session name.\n\
        2. `list_windows` and `inspect` to learn the window ID and sequences.\n\
        3. Drive the program: `typing` + `key` Enter for input, `wait_text` / \
        `wait_output` / `wait_screen_stable` for progress, `capture` to see the screen \
        as an image, `get_grid` for exact content assertions.\n\
        4. On failure, `inspect` before retrying: the application may be waiting for \
        input, the screen may already be stable, or no new frame may have presented.\n\
        5. `headless_stop` the session when done (ephemeral sessions also clean up \
        after disconnect, but stopping explicitly is tidier).\n\
        \n\
        A headless session has no OS focus, so pass `window_id` explicitly whenever it \
        holds more than one window, and expect `focus` to fail there."
    )
}

/// `prompts/list` result.
pub fn list() -> Value {
    json!({
        "prompts": PROMPTS.iter().map(|prompt| {
            json!({
                "name": prompt.name,
                "description": prompt.description,
                "arguments": prompt.arguments.iter().map(|(name, description, required)| {
                    json!({
                        "name": name,
                        "description": description,
                        "required": required,
                    })
                }).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
    })
}

fn debug_text(symptom: Option<&str>) -> String {
    let symptom = symptom
        .filter(|text| !text.trim().is_empty())
        .map(|text| format!("Symptom: {text}\n\n"))
        .unwrap_or_default();
    format!(
        "{symptom}Diagnose a misbehaving Vivido session without guessing:\n\
        \n\
        1. SNAPSHOT. Call `diagnose` for one correlated metadata-only view: window, \
        renderer, presenter, tracks, flow, connection health, plus the newest trace \
        events. Nothing waits; async metrics carry an age.\n\
        2. HISTORY. Query `vivid_trace` around the failure (`around` with \
        preceding/following, or `tail`), and `transcript` for the exact PTY bytes \
        that scrolled past. A `sequence_gap` means history was evicted: shrink the \
        window and re-read.\n\
        3. GAPS. If `drain_events` reports `overflow`, detail was collapsed: recover \
        with `inspect` plus `get_grid` before concluding anything.\n\
        4. REPRODUCE. Recreate the failure in an ephemeral headless session \
        (`headless_start`) so the repro is deterministic and screenshots are real \
        rendered frames; `headless_stop` it afterwards.\n\
        \n\
        Fix the root cause, not the symptom — and re-verify with `capture`."
    )
}

/// `prompts/get` result. `context` (tiers, attached session) is appended so the
/// workflow text reflects this server's actual capabilities.
pub fn get(name: &str, args: &Value, context: Option<&str>) -> Result<Value, String> {
    let prompt = PROMPTS
        .iter()
        .find(|prompt| prompt.name == name)
        .ok_or_else(|| format!("unknown prompt {name:?}"))?;
    let object = args.as_object().ok_or_else(|| "prompt arguments must be an object".to_owned())?;
    for key in object.keys() {
        if !prompt.arguments.iter().any(|(expected, _, _)| *expected == key) {
            return Err(format!("unknown argument `{key}` for prompt `{name}`"));
        }
    }
    for (expected, _, required) in prompt.arguments {
        if *required && args.get(expected).and_then(Value::as_str).is_none() {
            return Err(format!("prompt `{name}` needs argument `{expected}`"));
        }
    }
    let goal = args.get("goal").or_else(|| args.get("symptom")).and_then(Value::as_str);
    let mut text = (prompt.text)(goal);
    if let Some(context) = context {
        text.push_str(&format!("\n\nSession context: {context}"));
    }
    Ok(json!({
        "description": prompt.description,
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": text },
        }],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_round_trip() {
        let listed = list();
        assert_eq!(listed["prompts"].as_array().unwrap().len(), 3);
        let fetched = get("vivido_workflow", &json!({ "goal": "run ls" }), None).unwrap();
        let text = fetched["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("run ls"), "{text}");
        assert!(get("nope", &json!({}), None).is_err());
    }

    #[test]
    fn debug_prompt_takes_a_symptom_and_context() {
        let fetched =
            get("vivido_debug", &json!({ "symptom": "frozen screen" }), Some("observe-only"))
                .unwrap();
        let text = fetched["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("frozen screen"), "{text}");
        assert!(text.contains("observe-only"), "{text}");
    }

    #[test]
    fn prompt_arguments_must_be_known_and_an_object() {
        let error = get("vivido_workflow", &json!({"goal": "x", "bogus": 1}), None).unwrap_err();
        assert!(error.contains("bogus"), "{error}");
        let error = get("vivido_workflow", &json!([1, 2]), None).unwrap_err();
        assert!(error.contains("must be an object"), "{error}");
    }
}
