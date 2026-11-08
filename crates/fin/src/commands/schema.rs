//! `fin help <group>` and `fin schema <cmd>`, generated from the registry.

use fin_core::CoreError;
use serde_json::{Value, json};

use crate::output::usage;
use crate::registry::{commands, find, groups, names};

fn entry(c: &crate::registry::Command) -> Value {
    let mut e = json!({ "name": c.name, "usage": c.usage(), "summary": c.summary, "write": c.write, "input": (c.schema)() });
    let kinds = arg_kinds(c.name);
    if !kinds.is_empty() {
        e["argKinds"] = Value::Object(kinds);
    }
    e
}

/// `argKinds`: the command's arguments that name a client-side file (`path`), plan 7A 14c.
fn arg_kinds(name: &str) -> serde_json::Map<String, Value> {
    crate::agent_paths::path_args(name)
        .map(|a| (a.to_string(), json!("path")))
        .collect()
}

/// The named arguments of a built command: each flag's long and short spelling and how many
/// values it takes (`values`: 0 for a switch, `null` for an open count).
fn flags(cmd: &clap::Command) -> Vec<Value> {
    cmd.get_arguments()
        .filter(|a| !a.is_positional())
        .map(|a| {
            let values = if a.get_action().takes_values() {
                match a.get_num_args().map(|r| r.max_values()) {
                    Some(usize::MAX) => Value::Null,
                    Some(n) => json!(n),
                    None => json!(1),
                }
            } else {
                json!(0)
            };
            let mut f = json!({ "values": values });
            if let Some(long) = a.get_long() {
                f["long"] = json!(format!("--{long}"));
            }
            if let Some(short) = a.get_short() {
                f["short"] = json!(format!("-{short}"));
            }
            f
        })
        .collect()
}

/// `fin schema commands`: every command's name, whether it writes, its flags with their value
/// counts (the global ones included), and its path arguments with the flag (or the position)
/// that carries each, from the command line itself; `globals`: the flags that may come before
/// or between the command words. The built-in agent's `fin` tool reads it once per `fin` binary
/// (plan 7A Decisions 11, 14c) to find the command and its path values.
pub fn index(cli: &clap::Command) -> Value {
    let mut built = cli.clone();
    built.build();
    let list: Vec<Value> = commands()
        .map(|c| {
            let mut cmd = Some(&built);
            for word in c.name.split('.') {
                cmd = cmd.and_then(|x| x.find_subcommand(word));
            }
            let paths: Vec<Value> = crate::agent_paths::path_args(c.name)
                .map(|a| {
                    let arg = cmd.and_then(|x| x.get_arguments().find(|g| g.get_id() == a));
                    match arg.and_then(clap::Arg::get_long) {
                        Some(long) => json!({ "arg": a, "flag": format!("--{long}") }),
                        None => {
                            let position =
                                cmd.and_then(|x| x.get_positionals().position(|g| g.get_id() == a));
                            json!({ "arg": a, "position": position })
                        }
                    }
                })
                .collect();
            let flags = cmd.map(flags).unwrap_or_default();
            json!({ "name": c.name, "write": c.write, "paths": paths, "flags": flags })
        })
        .collect();
    json!({ "commands": list, "globals": flags(&built) })
}

/// The commands of a group (or a single command by name), with usage and input schema.
pub fn help(topic: &str) -> Result<Value, CoreError> {
    let commands: Vec<Value> = commands().filter(|c| c.group == topic).map(entry).collect();
    if !commands.is_empty() {
        return Ok(json!({ "group": topic, "commands": commands }));
    }
    if let Some(c) = find(&topic.replace(' ', ".")) {
        return Ok(json!({ "group": c.group, "commands": [entry(c)] }));
    }
    Err(usage(
        format!("no help group or command named {topic:?}"),
        json!({ "topic": topic, "alternatives": groups() }),
        "run `fin help <group>` with one of the listed groups",
    ))
}

/// The input JSON Schema of a command: `fin schema clip.insert` or `fin schema clip insert`.
pub fn schema(words: &[String]) -> Result<Value, CoreError> {
    let name = words.join(".");
    match find(&name) {
        Some(c) => Ok((c.schema)()),
        None => Err(usage(
            if name.is_empty() {
                "schema needs a command name".to_string()
            } else {
                format!("unknown command {name:?}")
            },
            json!({ "command": name, "alternatives": names() }),
            "run `fin schema <cmd>` with one of the listed commands, e.g. clip.insert",
        )),
    }
}

/// `fin schema param [--scope <s>] [--path <p>]`: the settings registry (plan 4C Task 24), every
/// spec with its type, range, unit, default, animatability and fix text. `scope` is a scope
/// (`item`, `effect`, `track`, `timeline`, `marker`, `style`), an item kind (`clip`, `caption`,
/// …) or an effect kind (`blur`, …); `path` keeps the paths it starts.
pub fn params(scope: Option<&str>, path: Option<&str>) -> Result<Value, CoreError> {
    use fin_core::params::{Scope, effect_kinds, specs};
    const SCOPES: &[&str] = &["item", "effect", "track", "timeline", "marker", "style"];
    const KINDS: &[&str] = &[
        "clip",
        "caption",
        "graphic",
        "image",
        "adjustment",
        "nested",
    ];
    if let Some(s) = scope
        && !SCOPES.contains(&s)
        && !KINDS.contains(&s)
        && !effect_kinds().contains(&s)
    {
        let mut alternatives: Vec<&str> = SCOPES.to_vec();
        alternatives.extend(KINDS);
        alternatives.extend(effect_kinds());
        return Err(usage(
            format!("no param scope {s:?}"),
            json!({ "scope": s, "alternatives": alternatives }),
            "pass --scope with a scope, an item kind or an effect kind",
        ));
    }
    let in_scope = |spec: &fin_core::params::ParamSpec| match (scope, spec.scope) {
        (None, _) => true,
        (Some("item"), Scope::Item { .. }) | (Some("effect"), Scope::Effect { .. }) => true,
        (Some("track"), Scope::Track)
        | (Some("timeline"), Scope::Timeline)
        | (Some("marker"), Scope::Marker)
        | (Some("style"), Scope::Style) => true,
        (Some(k), Scope::Item { kinds }) => kinds.contains(&k),
        (Some(k), Scope::Effect { kind }) => kind == k,
        _ => false,
    };
    let list: Vec<Value> = specs()
        .iter()
        .filter(|s| in_scope(s) && path.is_none_or(|p| s.path.starts_with(p)))
        .map(|s| json!(s))
        .collect();
    Ok(json!({ "count": list.len(), "params": list }))
}
