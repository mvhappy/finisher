//! `fin`: the agent-only Finisher CLI (spec §9). One JSON document on stdout, stable exit
//! codes, no prompts, pagers or colour.

mod addr;
mod agent_env;
mod agent_paths;
mod agent_session;
mod auth;
mod backend;
mod client;
mod client_fonts;
mod commands;
mod config;
mod conn;
mod deadline;
mod diag;
mod endpoint;
mod map;
mod output;
mod paths;
#[cfg(windows)]
mod pipe_client;
#[cfg(windows)]
mod private_windows;
mod registry;
mod registry_edit;
mod registry_library;
mod registry_plugins;
mod registry_rpcs;
mod registry_rpcs_edit;
mod registry_settings;
mod values;
mod view;

use std::ffi::OsString;
use std::io::Read;

use clap::error::{ContextKind, ContextValue};
use clap::{Command, CommandFactory, FromArgMatches, Parser, Subcommand};
use fin_core::CoreError;
use serde_json::{Map, Value, json};

use commands::agent::AgentCmd;
use commands::approval::ApprovalCmd;
use commands::asset::{AssetCmd, AssetEdit};
use commands::bin::BinCmd;
use commands::branch::BranchCmd;
use commands::cache::CacheCmd;
use commands::caption::CaptionCmd;
use commands::clip::ClipCmd;
use commands::diagnostics::DiagnosticsCmd;
use commands::effect::EffectCmd;
use commands::eval::EvalCmd;
use commands::fonts::{FontEdit, FontsCmd};
use commands::history::{CheckpointArgs, CherryPickArgs, MergeArgs, RevertArgs, UndoArgs};
use commands::item::ItemCmd;
use commands::job::{JobCmd, WaitArgs};
use commands::keyframe::KeyframeCmd;
use commands::library::LibraryCmd;
use commands::look::LookArgs;
use commands::marker::MarkerCmd;
use commands::media::{GraphicCmd, ImageCmd};
use commands::media_caps::MediaCmd;
use commands::param::ParamCmd;
use commands::plugin::{PluginCmd, RegistryCmd};
use commands::project::ProjectCmd;
use commands::search::SearchArgs;
use commands::style::StyleCmd;
use commands::timeline::TimelineCmd;
use commands::track::TrackCmd;
use commands::transition::TransitionCmd;
use commands::try_cmd::TryArgs;
use commands::view::ViewArgs;
use commands::{Edit, Globals, Write};

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
    #[command(subcommand)]
    Image(ImageCmd),
    #[command(subcommand)]
    Graphic(GraphicCmd),
    #[command(subcommand)]
    Item(ItemCmd),
    #[command(subcommand)]
    Transition(TransitionCmd),
    #[command(subcommand)]
    Track(TrackCmd),
    #[command(subcommand)]
    Timeline(TimelineCmd),
    #[command(subcommand)]
    Marker(MarkerCmd),
    #[command(subcommand)]
    Bin(BinCmd),
    #[command(subcommand)]
    Param(ParamCmd),
    #[command(subcommand)]
    Keyframe(KeyframeCmd),
    #[command(subcommand)]
    Effect(EffectCmd),
    #[command(subcommand)]
    Caption(CaptionCmd),
    #[command(subcommand)]
    Style(StyleCmd),
    #[command(subcommand)]
    Eval(EvalCmd),
    Look(LookArgs),
    #[command(subcommand)]
    Job(JobCmd),
    Wait(WaitArgs),
    #[command(subcommand)]
    Approval(ApprovalCmd),
    #[command(subcommand)]
    Agent(AgentCmd),
    #[command(subcommand)]
    Library(LibraryCmd),
    Search(SearchArgs),
    #[command(subcommand)]
    Media(MediaCmd),
    #[command(subcommand)]
    Cache(CacheCmd),
    #[command(subcommand)]
    Fonts(FontsCmd),
    #[command(subcommand)]
    Diagnostics(DiagnosticsCmd),
    #[command(subcommand)]
    Plugin(PluginCmd),
    #[command(subcommand)]
    Registry(RegistryCmd),
    #[command(subcommand)]
    Branch(BranchCmd),
    Undo(UndoArgs),
    Redo,
    Checkpoint(CheckpointArgs),
    Merge(MergeArgs),
    CherryPick(CherryPickArgs),
    Revert(RevertArgs),
    Try(TryArgs),
    Batch,
    Whoami,
    Help {
        topic: Option<String>,
    },
    Schema {
        command: Vec<String>,
        /// `fin schema param` only: a scope, item kind or effect kind.
        #[arg(long, value_name = "scope")]
        scope: Option<String>,
        /// `fin schema param` only: keep the paths it starts.
        #[arg(long, value_name = "prefix")]
        path: Option<String>,
        /// Every command, the RPCs it sends and what identifies it (committed as
        /// `packages/contracts/registry.json`).
        #[arg(long)]
        registry: bool,
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

/// The parsed command line and the registry name of its command.
fn parse_named(args: Vec<OsString>) -> Result<(Cli, Option<String>), CoreError> {
    let mut cmd = configure(Cli::command());
    let matches = cmd
        .try_get_matches_from_mut(&args)
        .map_err(|e| clap_usage(&e))?;
    diag::note_matches(&cmd, &matches);
    let cli = Cli::from_arg_matches(&matches).map_err(|e| clap_usage(&e))?;
    let name = agent_env::command_name(&matches);
    // Under an agent credential, file arguments are confined before anything opens them (14c).
    agent_paths::check_project(cli.globals.project.as_deref())?;
    agent_paths::check_matches(&matches, name.as_deref(), cli.globals.project.as_deref())?;
    Ok((cli, name))
}

#[cfg(test)]
fn parse(args: Vec<OsString>) -> Result<Cli, CoreError> {
    parse_named(args).map(|(cli, _)| cli)
}

fn run(args: Vec<OsString>) -> Result<Value, CoreError> {
    let (mut cli, name) = parse_named(args)?;
    agent_env::apply(&mut cli.globals, name.as_deref())?;
    let g = &cli.globals;
    let backend = || backend::select(g.project.as_deref());
    match cli.command {
        None | Some(Cmd::Help { topic: None }) => Ok(map::map()),
        Some(Cmd::Help { topic: Some(t) }) => commands::schema::help(&t),
        Some(Cmd::Schema {
            command,
            scope,
            path,
            registry,
        }) => match command.as_slice() {
            [] if registry && scope.is_none() && path.is_none() => {
                Ok(commands::schema_registry::registry())
            }
            _ if registry => Err(output::usage(
                "--registry takes no command, --scope or --path",
                json!({ "command": command }),
                "run fin schema --registry alone",
            )),
            [p] if p == "param" => commands::schema::params(scope.as_deref(), path.as_deref()),
            [c] if c == "commands" && scope.is_none() && path.is_none() => {
                Ok(commands::schema::index(&configure(Cli::command())))
            }
            _ if scope.is_some() || path.is_some() => Err(output::usage(
                "--scope and --path belong to fin schema param",
                json!({ "command": command }),
                "run fin schema param [--scope <s>] [--path <p>], or fin schema <cmd> alone",
            )),
            _ => commands::schema::schema(&command),
        },
        Some(Cmd::Status) => commands::view::status(backend()?.as_ref(), g),
        Some(Cmd::View(a)) => commands::view::view(backend()?.as_ref(), g, &a),
        Some(Cmd::Project(ProjectCmd::Create(a))) => {
            commands::project::create(backend()?.as_ref(), g, &a)
        }
        // Not a write: `--dry-run` would be silently ignored, so it is refused (SP2 follow-up).
        Some(Cmd::Project(ProjectCmd::Open(_))) if g.dry_run => Err(output::usage(
            "project open is not a write and takes no --dry-run",
            json!({ "command": "project.open", "flag": "dry-run" }),
            "run fin project open <dir> without --dry-run (it only registers the folder)",
        )),
        Some(Cmd::Project(ProjectCmd::Open(a))) => commands::project::open(backend()?.as_ref(), &a),
        Some(Cmd::Project(cmd)) => commands::project::storage(backend()?.as_ref(), g, &cmd),
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
        Some(Cmd::Asset(AssetCmd::Move(a))) => edit(g, Edit::Asset(AssetEdit::Move(a))),
        Some(Cmd::Asset(AssetCmd::Remove(a))) => edit(g, Edit::Asset(AssetEdit::Remove(a))),
        Some(Cmd::Image(c)) => edit(g, Edit::Image(c)),
        Some(Cmd::Graphic(c)) => edit(g, Edit::Graphic(c)),
        Some(Cmd::Item(c)) => edit(g, Edit::Item(c)),
        Some(Cmd::Transition(c)) => edit(g, Edit::Transition(c)),
        Some(Cmd::Track(c)) => edit(g, Edit::Track(c)),
        Some(Cmd::Timeline(c)) => edit(g, Edit::Timeline(c)),
        Some(Cmd::Marker(c)) => edit(g, Edit::Marker(c)),
        Some(Cmd::Bin(c)) => edit(g, Edit::Bin(c)),
        Some(Cmd::Param(c)) => edit(g, Edit::Param(c)),
        Some(Cmd::Keyframe(c)) => edit(g, Edit::Keyframe(c)),
        Some(Cmd::Effect(c)) => edit(g, Edit::Effect(c)),
        Some(Cmd::Caption(c)) => edit(g, Edit::Caption(c)),
        Some(Cmd::Style(StyleCmd::Ls)) => commands::style::ls(backend()?.as_ref(), g),
        Some(Cmd::Style(c)) => edit(g, Edit::Style(c)),
        Some(Cmd::Eval(c)) => commands::eval::run(backend()?.as_ref(), g, &c),
        Some(Cmd::Look(a)) => commands::look::run(backend()?.as_ref(), g, &a),
        Some(Cmd::Job(c)) => commands::job::run(backend()?.as_ref(), &c),
        Some(Cmd::Wait(a)) => commands::job::wait(backend()?.as_ref(), &a),
        Some(Cmd::Approval(c)) => commands::approval::run(backend()?.as_ref(), &c),
        Some(Cmd::Agent(c)) => commands::agent::run(backend()?.as_ref(), &c, g.project.as_deref()),
        Some(Cmd::Library(c)) => commands::library::run(backend()?.as_ref(), &c),
        Some(Cmd::Search(a)) => commands::search::run(backend()?.as_ref(), &a),
        Some(Cmd::Media(c)) => commands::media_caps::run(backend()?.as_ref(), &c),
        Some(Cmd::Cache(c)) => commands::cache::run(backend()?.as_ref(), &c),
        Some(Cmd::Fonts(FontsCmd::Pin(a))) => edit(g, Edit::Font(FontEdit::Pin(a))),
        Some(Cmd::Fonts(FontsCmd::Repin(a))) => edit(g, Edit::Font(FontEdit::Repin(a))),
        Some(Cmd::Fonts(FontsCmd::Unpin(a))) => edit(g, Edit::Font(FontEdit::Unpin(a))),
        Some(Cmd::Fonts(c)) => commands::fonts::read(backend()?.as_ref(), g, &c),
        Some(Cmd::Diagnostics(c)) => commands::diagnostics::run(backend()?.as_ref(), g, &c),
        Some(Cmd::Plugin(c)) => commands::plugin::plugin(backend()?.as_ref(), &c),
        Some(Cmd::Registry(c)) => commands::plugin::registry(backend()?.as_ref(), &c),
        Some(Cmd::Branch(cmd)) => commands::branch::run(backend()?.as_ref(), g, &cmd),
        Some(Cmd::Undo(a)) => commands::history::undo(backend()?.as_ref(), g, &a),
        Some(Cmd::Redo) => commands::history::redo(backend()?.as_ref(), g),
        Some(Cmd::Checkpoint(a)) => commands::history::checkpoint(backend()?.as_ref(), g, &a),
        Some(Cmd::Merge(a)) => commands::history::merge(backend()?.as_ref(), g, &a),
        Some(Cmd::CherryPick(a)) => commands::history::cherry_pick(backend()?.as_ref(), g, &a),
        Some(Cmd::Revert(a)) => commands::history::revert(backend()?.as_ref(), g, &a),
        Some(Cmd::Try(a)) => commands::try_cmd::run(backend()?.as_ref(), g, &a),
        Some(Cmd::Whoami) => commands::whoami::run(),
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

/// Runs one 4C edit command.
fn edit(g: &Globals, step: Edit) -> Result<Value, CoreError> {
    let backend = backend::select(g.project.as_deref())?;
    commands::write(backend.as_ref(), g, Write::Edit(step))
}

fn main() {
    diag::begin();
    output::install_panic_hook();
    // `FIN_TEST_PANIC` (test builds only) before anything else; the overall deadline
    // (5A Decision 36) bounds the command, not D6's recording, which has its own budget.
    diag::test_panic();
    let result = deadline::start().and_then(|()| run(std::env::args_os().collect()));
    let code = match &result {
        Ok(doc) => output::ok(doc),
        Err(e) => output::error(e),
    };
    diag::finish(&result, code);
    std::process::exit(code);
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
