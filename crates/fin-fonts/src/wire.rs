//! What the server sends about fonts (4C Decisions 34 and 35): the `fonts.status` and
//! `fonts.plan` answers. Data only; `fin-node` fills them. Exported to TypeScript through
//! `fin-store`'s `export_schema`.

use fin_core::{CoreError, Op};
use schemars::JsonSchema;
use serde::Serialize;

use crate::FileState;

/// `fonts.status`: every catalog file's state in this server, then the files pinned by open
/// projects that the catalog does not list (font assets, files of another catalog).
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontsStatus {
    pub catalog_version: String,
    /// Background fetching is on (`fonts.autoFetch`, and `FIN_FONTS_OFFLINE` unset).
    pub auto_fetch: bool,
    /// The font cache or the catalog could not be used (`FONTS_DAMAGED`): nothing is
    /// fetched, and only font assets measure.
    #[schemars(required, with = "Option<CoreError>")]
    pub damaged: Option<CoreError>,
    pub files: Vec<FontFileStatus>,
}

/// One font file. `entry` and `style` name its catalog style, when the catalog lists it;
/// `keys` are the font keys (`<family>|<weight>|<normal|italic>`) open projects pin it as.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontFileStatus {
    pub sha256: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub entry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub style: Option<String>,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub keys: Vec<String>,
    pub state: FileState,
}

/// `fonts.plan`: the `font.pin` ops a client prepends to its tx, and notes about faces or
/// scripts the catalog cannot pin (`FONT_NOT_IN_CATALOG`), which never fail the plan.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontsPlan {
    pub ops: Vec<Op>,
    pub notes: Vec<CoreError>,
}
