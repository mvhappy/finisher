//! The 5D `library` commands' registry entries (plan 5D Task 14), listed after the job commands
//! in [`crate::registry::commands`]. `fin library set` lists the human-only fields too: `fin`
//! sends them and the server refuses them with `HUMAN_ONLY` (exit 7).

use crate::commands::library::{self as lib, RefArgs};
use crate::commands::library_set::SetArgs;
use crate::commands::{project, search};
use crate::registry::{Command, schema_of};

const fn cmd(
    name: &'static str,
    group: &'static str,
    args: &'static str,
    summary: &'static str,
    schema: fn() -> serde_json::Value,
) -> Command {
    Command {
        name,
        group,
        args,
        summary,
        write: false,
        schema,
    }
}

pub const LIBRARY_COMMANDS: &[Command] = &[
    cmd(
        "library.ls",
        "library",
        "",
        "the server's libraries: state, counts, warnings",
        || schema_of::<project::NoArgs>("library.ls"),
    ),
    cmd(
        "library.status",
        "library",
        "<lib>",
        "one library: scans running, stage progress, watch (a library is an id or a name)",
        || schema_of::<RefArgs>("library.status"),
    ),
    cmd(
        "library.scan",
        "library",
        "<lib> [--full] [--dir <path>]…",
        "queue a scan and return its job (fin wait <job>); LIBRARY_PAUSED/OFFLINE say why",
        || schema_of::<lib::ScanArgs>("library.scan"),
    ),
    cmd(
        "library.files",
        "library",
        "<lib> [<dir>] [--state <s>] [--kind <k>] [--limit <n>] [--cursor <c>]",
        "one folder's subfolders and files; next is the next page's cursor",
        || schema_of::<lib::FilesArgs>("library.files"),
    ),
    cmd(
        "library.show",
        "library",
        "<lib> (<path> | --id <fileId>)",
        "one file with probe and stages; twins are PATH_AMBIGUOUS (use --id)",
        || schema_of::<lib::ShowArgs>("library.show"),
    ),
    cmd(
        "library.search",
        "library",
        "[<query>] [--lib <lib>]… [--kind <k>] [--min-duration <t>] [--max-duration <t>] [--min-width <px>] [--min-height <px>] [--since <date>] [--state <s>] [--limit <n>] [--cursor <c>]",
        "names and folders across libraries, ranked; --cursor takes next",
        || schema_of::<search::SearchArgs>("library.search"),
    ),
    cmd(
        "library.set",
        "library",
        "<lib> [--name <n>] [--include <g>]… [--exclude <g>]… [--kinds all|<k>,…] [--poll demand|<s>] [--full daily|weekly|manual] [--pause|--resume]",
        "change a library's filters, kinds, schedule or pause (--hydrate, --bandwidth, --alias, --accept-*: human-only, HUMAN_ONLY exit 7)",
        || schema_of::<SetArgs>("library.set"),
    ),
    cmd(
        "search",
        "library",
        "<query> [--lib <lib>]… [--kind <k>] [--min-duration <t>] [--since <date>] [--limit <n>] [--cursor <c>]",
        "fin library search: names and folders across libraries, ranked",
        || schema_of::<search::SearchArgs>("search"),
    ),
];
