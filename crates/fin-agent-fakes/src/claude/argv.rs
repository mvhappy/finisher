//! The command line, parsed like the CLI's option parser (commander): `--flag value`,
//! `--flag=value`, short aliases, and `error: ...` messages for what it refuses.

use std::collections::BTreeMap;

pub type Refusals = BTreeMap<String, Vec<String>>;

use serde_json::{Value, json};

pub const BOOLS: &[&str] = &[
    "--print",
    "--verbose",
    "--include-partial-messages",
    "--replay-user-messages",
    "--strict-mcp-config",
    "--disable-slash-commands",
    "--restricted",
    "--safe-mode",
    "--bare",
    "--version",
];
pub const VALUES: &[&str] = &[
    "--input-format",
    "--output-format",
    "--permission-mode",
    "--permission-prompts",
    "--permission-prompt-tool",
    "--setting-sources",
    "--settings",
    "--mcp-config",
    "--tools",
    "--disallowedTools",
    "--allowedTools",
    "--append-system-prompt-file",
    "--append-system-prompt",
    "--system-prompt",
    "--model",
    "--session-id",
    "--resume",
    "--max-budget-usd",
    "--max-turns",
    "--add-dir",
];
const MODES: &[&str] = &[
    "default",
    "manual",
    "acceptEdits",
    "plan",
    "auto",
    "dontAsk",
    "bypassPermissions",
];

#[derive(Debug, PartialEq)]
pub enum Command {
    Version,
    AuthStatus { text: bool },
    Print,
}

#[derive(Debug)]
pub struct Args {
    pub command: Command,
    pub flags: BTreeMap<String, Value>,
}

impl Args {
    pub fn str(&self, flag: &str) -> Option<&str> {
        self.flags.get(flag).and_then(Value::as_str)
    }
    pub fn flags_json(&self) -> Value {
        json!(self.flags)
    }
}

fn canonical(flag: &str) -> &str {
    match flag {
        "-p" => "--print",
        "-v" | "-V" => "--version",
        "-r" => "--resume",
        "--disallowed-tools" => "--disallowedTools",
        "--allowed-tools" => "--allowedTools",
        other => other,
    }
}

pub fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Parses `args`; `refuse` names flag values this version does not know.
pub fn parse(args: &[String], refuse: &Refusals) -> Result<Args, String> {
    if args.first().map(String::as_str) == Some("auth") {
        let mut text = false;
        let mut seen_status = false;
        for a in &args[1..] {
            match a.as_str() {
                "status" => seen_status = true,
                "--json" => text = false,
                "--text" => text = true,
                other if other.starts_with('-') => {
                    return Err(format!("error: unknown option '{other}'"));
                }
                other => return Err(format!("error: unknown command '{other}'")),
            }
        }
        if !seen_status {
            return Err("error: fake-claude supports only `auth status [--json|--text]`".into());
        }
        return Ok(Args {
            command: Command::AuthStatus { text },
            flags: BTreeMap::new(),
        });
    }
    let mut flags = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if !a.starts_with('-') || a == "-" {
            continue; // a positional prompt
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        let name = canonical(name);
        if BOOLS.contains(&name) && inline.is_none() {
            flags.insert(name.to_string(), Value::Bool(true));
        } else if VALUES.contains(&name) {
            let value = match inline {
                Some(v) => v,
                None if i < args.len() => {
                    i += 1;
                    args[i - 1].clone()
                }
                None => return Err(format!("error: option '{name} <value>' argument missing")),
            };
            match flags.get_mut(name) {
                // A repeated value flag (`--add-dir a --add-dir b`) keeps every value.
                Some(Value::Array(all)) => all.push(Value::String(value)),
                Some(first) if name == "--add-dir" => {
                    *first = Value::Array(vec![first.take(), Value::String(value)]);
                }
                _ => {
                    flags.insert(name.to_string(), Value::String(value));
                }
            }
        } else {
            return Err(format!("error: unknown option '{a}'"));
        }
    }
    let args = Args {
        command: Command::Print,
        flags,
    };
    validate(&args)?;
    for (flag, values) in refuse {
        if let Some(v) = args.str(flag)
            && values.iter().any(|r| r == v)
        {
            return Err(refused_value(flag, v));
        }
    }
    let command = if args.flags.contains_key("--version") {
        Command::Version
    } else {
        Command::Print
    };
    Ok(Args {
        command,
        flags: args.flags,
    })
}

fn choice(args: &Args, flag: &str, allowed: &[&str]) -> Result<(), String> {
    match args.str(flag) {
        Some(v) if !allowed.contains(&v) => Err(format!(
            "error: option '{flag} <value>' argument '{v}' is invalid. Allowed choices are {}.",
            allowed.join(", ")
        )),
        _ => Ok(()),
    }
}

fn validate(args: &Args) -> Result<(), String> {
    if args.flags.contains_key("--version") {
        return Ok(());
    }
    choice(args, "--input-format", &["text", "stream-json"])?;
    choice(args, "--output-format", &["text", "json", "stream-json"])?;
    choice(args, "--permission-mode", MODES)?;
    choice(args, "--permission-prompts", &["host", "none"])?;
    if let Some(v) = args.str("--setting-sources") {
        for s in v.split(',') {
            if !["user", "project", "local"].contains(&s) {
                return Err(format!(
                    "Error processing --setting-sources: Invalid setting source: {s}. Valid sources: user, project, local"
                ));
            }
        }
    }
    for flag in ["--session-id", "--resume"] {
        if let Some(v) = args.str(flag)
            && !is_uuid(v)
        {
            return Err(format!("Error: {flag} must be a valid UUID, got '{v}'"));
        }
    }
    if let Some(v) = args.str("--max-budget-usd")
        && !v.parse::<f64>().is_ok_and(|n| n.is_finite() && n >= 0.0)
    {
        return Err("Error: --max-budget-usd must be a number greater than or equal to 0".into());
    }
    let print = args.flags.contains_key("--print");
    let stream_out = args.str("--output-format") == Some("stream-json");
    if !print {
        return Err("Error: fake-claude supports only print mode (-p)".into());
    }
    if stream_out && !args.flags.contains_key("--verbose") {
        return Err(
            "Error: When using --print, --output-format=stream-json requires --verbose".into(),
        );
    }
    if args.flags.contains_key("--include-partial-messages") && !stream_out {
        return Err(
            "Error: --include-partial-messages requires --print and --output-format=stream-json"
                .into(),
        );
    }
    if args.flags.contains_key("--replay-user-messages")
        && (args.str("--input-format") != Some("stream-json") || !stream_out)
    {
        return Err("Error: --replay-user-messages requires --input-format=stream-json and --output-format=stream-json".into());
    }
    if args.flags.contains_key("--restricted")
        && args.str("--permission-mode") == Some("bypassPermissions")
    {
        return Err("Error: --restricted refuses --permission-mode bypassPermissions".into());
    }
    Ok(())
}

/// The error a version that lacks `value` gives: `--permission-prompt-tool` takes an MCP tool
/// name, so an unknown value is a missing tool; other flags refuse it as a choice.
fn refused_value(flag: &str, value: &str) -> String {
    if flag == "--permission-prompt-tool" {
        format!(
            "Error: MCP tool {value} (passed via --permission-prompt-tool) not found. Available MCP tools: none"
        )
    } else {
        format!("error: option '{flag} <value>' argument '{value}' is invalid.")
    }
}
