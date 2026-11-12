//! The edit commands' registry rows (plan 7B Decision 1): every one commits a `tx.commit`, and
//! the transaction's ops are all the server sees, so a command is singled out by the op names
//! (`tx.ops.*.op`) and, for an `item.insert`, the item's `kind`. Commands that commit the same
//! ops (`clip.trim` and `item.trim`, `clip.delete` and `item.delete`) list the same entries.
//! fin-core also takes the `clip.insert`, `clip.trim` and `clip.delete` alias ops on the wire
//! (`fin_core::ops::alias`) and rewrites them only in the engine, so the rows of the ops they
//! stand for name them too (`every_client_op_is_named_by_a_row`).
//! `batch` has no condition: the server names it for any transaction no other row singles out,
//! and for every transaction of several ops.
//! Plugin commands (sp8 §3.8) are named by the server from the plugin host's own record of the
//! calling plugin, never from the payload.

use crate::registry_rpcs::{Match, Row, RpcUse, row, rpc_if};

const OP: &str = "tx.ops.*.op";
const KIND: &str = "tx.ops.*.kind";
const TX: &[&str] = &["projectId", "tx.branch", "tx.ops"];

/// A `tx.commit` that holds the op `$op`.
macro_rules! op {
    ($op:literal) => {
        rpc_if!("tx.commit", OP => Match::Eq($op))
    };
}
/// A `tx.commit` that holds the op `$op` or its `clip.*` alias `$alias`.
macro_rules! op_or_alias {
    ($op:literal, $alias:literal) => {
        &[
            RpcUse {
                method: "tx.commit",
                matches: &[(OP, Match::Eq($op))],
            },
            RpcUse {
                method: "tx.commit",
                matches: &[(OP, Match::Eq($alias))],
            },
        ]
    };
}
/// A `tx.commit` that holds an `item.insert` of the given kind.
macro_rules! insert {
    ($kind:literal) => {
        rpc_if!("tx.commit", OP => Match::Eq("item.insert"), KIND => Match::Eq($kind))
    };
}

pub const EDIT_ROWS: &[Row] = &[
    row!("asset.add", op!("asset.add"), TX),
    row!(
        "clip.insert",
        &[
            RpcUse {
                method: "tx.commit",
                matches: &[(OP, Match::Eq("item.insert")), (KIND, Match::Eq("clip"))]
            },
            RpcUse {
                method: "tx.commit",
                matches: &[(OP, Match::Eq("clip.insert"))]
            },
        ],
        TX
    ),
    row!("clip.trim", op_or_alias!("item.trim", "clip.trim"), TX),
    row!(
        "clip.delete",
        op_or_alias!("item.delete", "clip.delete"),
        TX
    ),
    row!("image.insert", insert!("image"), TX),
    row!("graphic.insert", insert!("graphic"), TX),
    row!("item.move", op!("item.move"), TX),
    row!("item.split", op!("item.split"), TX),
    row!(
        "item.delete",
        op_or_alias!("item.delete", "clip.delete"),
        TX
    ),
    row!("item.trim", op_or_alias!("item.trim", "clip.trim"), TX),
    row!("item.slip", op!("item.slip"), TX),
    row!("item.slide", op!("item.slide"), TX),
    row!("item.link", op!("item.link"), TX),
    row!("item.unlink", op!("item.unlink"), TX),
    row!("transition.set", op!("transition.set"), TX),
    row!("transition.remove", op!("transition.remove"), TX),
    row!("track.add", op!("track.add"), TX),
    row!("track.remove", op!("track.remove"), TX),
    row!("track.move", op!("track.move"), TX),
    row!("track.set", op!("track.set"), TX),
    row!("timeline.add", op!("timeline.add"), TX),
    row!("timeline.set", op!("timeline.set"), TX),
    row!("timeline.remove", op!("timeline.remove"), TX),
    row!("marker.add", op!("marker.add"), TX),
    row!("marker.set", op!("marker.set"), TX),
    row!("marker.remove", op!("marker.remove"), TX),
    row!("bin.add", op!("bin.add"), TX),
    row!("bin.move", op!("bin.move"), TX),
    row!("bin.remove", op!("bin.remove"), TX),
    row!("asset.move", op!("asset.move"), TX),
    row!("asset.remove", op!("asset.remove"), TX),
    row!("param.set", op!("param.set"), TX),
    row!("keyframe.set", op!("keyframe.set"), TX),
    row!("keyframe.remove", op!("keyframe.remove"), TX),
    row!("effect.add", op!("effect.add"), TX),
    row!("effect.remove", op!("effect.remove"), TX),
    row!("effect.move", op!("effect.move"), TX),
    row!("caption.import", insert!("caption"), TX),
    row!("caption.set-text", op!("caption.setText"), TX),
    row!("caption.split", op!("caption.split"), TX),
    row!("caption.merge", op!("caption.merge"), TX),
    row!("caption.style", op!("caption.setStyle"), TX),
    row!("style.add", op!("style.add"), TX),
    row!("style.set", op!("style.set"), TX),
    row!("style.remove", op!("style.remove"), TX),
    row!("fonts.pin", op!("font.pin"), TX),
    row!("fonts.repin", op!("font.repin"), TX),
    row!("fonts.unpin", op!("font.unpin"), TX),
    row!("batch", crate::registry_rpcs::rpc!("tx.commit"), TX),
];

/// The name pattern of a plugin command, exported as the last registry entry. `fin @<id>
/// <command>` sends `plugin.command.run {name: "plugin:<id>/<command>"}`, named by that `name`
/// (the host runs exactly that command or none); the transaction it commits is a `tx.commit`
/// the server names from the plugin host's own record (`{plugin: "<id>@<version>", command}`),
/// never from a payload's `tx.via`. Neither has a payload condition here.
pub const PLUGIN_COMMAND: &str = "plugin:<id>/<command>";
pub const PLUGIN_COMMAND_RPCS: &[RpcUse] = &[
    RpcUse {
        method: "plugin.command.run",
        matches: &[],
    },
    RpcUse {
        method: "tx.commit",
        matches: &[],
    },
];
