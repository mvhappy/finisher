//! `fin`: the agent-only Finisher CLI (spec §9). One JSON document on stdout, stable exit
//! codes, no prompts, pagers or colour.

mod addr;
mod backend;
mod client;
mod commands;
mod conn;
mod endpoint;
mod map;
mod output;
mod paths;
mod registry;

use std::ffi::OsString;
use std::io::Read;

use clap::error::{ContextKind, ContextValue};
use clap::{Command, CommandFactory, FromArgMatches, Parser, Subcommand};
use fin_core::CoreError;
use serde_json::{Map, Value, json};

use commands::asset::AssetCmd;
use commands::clip::ClipCmd;
use commands::project::ProjectCmd;
use commands::view::ViewArgs;
use commands::{Globals, Write};

#[derive(Debug, Parser)]
#[command(name = "fin")]
struct Cli {
    #[command(flatten)]
    globals: Globals,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    #[command(subcommand)]
    Project(ProjectCmd),
    Status,
    View(ViewArgs),
    #[command(subcommand)]
    Asset(AssetCmd),
    #[command(subcommand)]
    Clip(ClipCmd),
    Batch,
    Help {
        topic: Option<String>,
    },
    Schema {
        command: Vec<String>,
    },
}

/// No clap help/version flags, help subcommand or help-on-missing-subcommand anywhere in the
/// tree: every parse problem becomes a `USAGE` document. Colour is compiled out (clap is built
/// without its `color` feature, so the colour choice is always `Never`).
fn configure(cmd: Command) -> Command {
    cmd.disable_help_flag(true)
        .disable_version_flag(true)
        .disable_help_subcommand(true)
        .arg_required_else_help(false)
        .mut_subcommands(configure)
}

fn context_value(v: &ContextValue) -> Value {
    match v {
        ContextValue::None => Value::Null,
        ContextValue::Bool(b) => json!(b),
        ContextValue::String(s) => json!(s),
        ContextValue::Strings(s) => json!(s),
        ContextValue::StyledStr(s) => json!(s.to_string()),
        ContextValue::StyledStrs(s) => json!(s.iter().map(ToString::to_string).collect::<Vec<_>>()),
        ContextValue::Number(n) => json!(n),
        _ => json!(v.to_string()),
    }
}

fn clap_usage(err: &clap::Error) -> CoreError {
    let rendered = err.render().to_string();
    let message = rendered
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim_start_matches("error: ").to_string())
        .unwrap_or_else(|| format!("{:?}", err.kind()));
    let usage_line = rendered
        .lines()
        .find_map(|l| l.trim().strip_prefix("Usage: "))
        .map(str::to_string);
    let mut context = Map::new();
    for (kind, value) in err.context() {
        let value = context_value(value);
        if kind == ContextKind::Usage || value.as_array().is_some_and(Vec::is_empty) {
            continue;
        }
        let name = format!("{kind:?}");
        let mut chars = name.chars();
        let key = chars
            .next()
            .map(|c| c.to_ascii_lowercase().to_string() + chars.as_str())
            .unwrap_or_default();
        context.insert(key, value);
    }
    let mut details = json!({ "kind": format!("{:?}", err.kind()), "context": context });
    let suggested = [
        ContextKind::SuggestedArg,
        ContextKind::SuggestedSubcommand,
        ContextKind::ValidSubcommand,
    ]
    .iter()
    .filter_map(|k| err.get(*k).map(context_value))
    .find(|v| v.as_array().is_none_or(|a| !a.is_empty()));
    if let Some(alternatives) = suggested {
        details["alternatives"] = alternatives;
    }
    let fix = match &usage_line {
        Some(u) => format!("usage: {u}; run `fin` for the command map"),
        None => "run `fin` for the command map or `fin help <group>`".to_string(),
    };
    output::usage(message, details, fix)
}

fn parse(args: Vec<OsString>) -> Result<Cli, CoreError> {
    let matches = configure(Cli::command())
        .try_get_matches_from(args)
        .map_err(|e| clap_usage(&e))?;
    Cli::from_arg_matches(&matches).map_err(|e| clap_usage(&e))
}

fn run(args: Vec<OsString>) -> Result<Value, CoreError> {
    let cli = parse(args)?;
    let g = &cli.globals;
    let backend = || backend::select(g.project.as_deref());
    match cli.command {
        None | Some(Cmd::Help { topic: None }) => Ok(map::map()),
        Some(Cmd::Help { topic: Some(t) }) => commands::schema::help(&t),
        Some(Cmd::Schema { command }) => commands::schema::schema(&command),
        Some(Cmd::Status) => commands::view::status(backend()?.as_ref()),
        Some(Cmd::View(a)) => commands::view::view(backend()?.as_ref(), &a),
        Some(Cmd::Project(ProjectCmd::Create(a))) => {
            commands::project::create(backend()?.as_ref(), g, &a)
        }
        Some(Cmd::Project(ProjectCmd::Open(a))) => commands::project::open(backend()?.as_ref(), &a),
        Some(Cmd::Project(cmd)) => commands::project::storage(backend()?.as_ref(), &cmd),
        Some(Cmd::Asset(AssetCmd::Add(a))) => {
            commands::write(backend()?.as_ref(), g, Write::AssetAdd(a))
        }
        Some(Cmd::Clip(ClipCmd::Insert(a))) => {
            commands::write(backend()?.as_ref(), g, Write::ClipInsert(a))
        }
        Some(Cmd::Clip(ClipCmd::Trim(a))) => {
            commands::write(backend()?.as_ref(), g, Write::ClipTrim(a))
        }
        Some(Cmd::Clip(ClipCmd::Delete(a))) => {
            commands::write(backend()?.as_ref(), g, Write::ClipDelete(a))
        }
        Some(Cmd::Batch) => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input).map_err(|e| {
                output::usage(
                    format!("cannot read stdin: {e}"),
                    json!({}),
                    "pipe UTF-8 JSONL to `fin batch`",
                )
            })?;
            commands::batch(backend()?.as_ref(), g, &input)
        }
    }
}

fn main() {
    output::install_panic_hook();
    let code = match run(std::env::args_os().collect()) {
        Ok(doc) => output::ok(&doc),
        Err(e) => output::error(&e),
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<OsString> {
        std::iter::once("fin")
            .chain(s.split_whitespace())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn every_registry_command_parses() {
        let samples = [
            "project create d --name n",
            "project open d --relocate",
            "project status",
            "project takeover --confirm",
            "project verify",
            "project rebuild",
            "project compact",
            "project recover",
            "project acknowledge --rewind 2 --branch main",
            "status",
            "view",
            "asset add f.mp4",
            "clip insert --track t1 --at end --asset a1 --dur 1s",
            "clip trim c1 --end -1s",
            "clip delete c1",
            "batch",
            "help clip",
            "schema clip.insert",
        ];
        assert_eq!(samples.len(), registry::COMMANDS.len());
        for (sample, command) in samples.iter().zip(registry::COMMANDS) {
            assert!(
                sample.starts_with(&command.name.replace('.', " ")),
                "{sample} vs {}",
                command.name
            );
            assert!(parse(args(sample)).is_ok(), "{sample}");
        }
    }

    #[test]
    fn colour_is_never_used() {
        assert_eq!(
            configure(Cli::command()).get_color(),
            clap::ColorChoice::Never
        );
    }

    #[test]
    fn help_flags_are_usage_errors() {
        for s in ["--help", "clip insert --help", "-h", "--version", "clip"] {
            assert_eq!(parse(args(s)).unwrap_err().code, "USAGE", "{s}");
        }
    }
}
