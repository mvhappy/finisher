//! `media.thumbs` v1 (plan 6B-1 Task 15, Decision 17; sub-project 6 spec §3.2 item 6):
//! thumbnails at chosen source times and filmstrips, as PNGs in one cache entry, made by
//! `fin-workers media` and committed into the media cache by the host.
//!
//! - **Thumbnails** ([`ThumbsMode::Thumbs`]): one PNG per requested time (1 to
//!   [`MAX_THUMBS`], `t<i>.png`), the frame `frame_at` picks on the *original's* sample table
//!   (spec §10.2: midpoint ties go to the earlier frame), shown upright (rotation and pixel
//!   aspect applied: [`thumb_size`] fits the display size's longest side to `maxSide`, never
//!   above the source).
//! - **Filmstrips** ([`ThumbsMode::Strip`]): one sprite PNG per [`STRIP_SPRITE_SECONDS`] s of
//!   source (`s<k>.png`), a tile per second ([`STRIP_TILE_SECONDS`]) side by side, each
//!   `tileHeight` tall ([`tile_size`]) and sampled at its second's start; `strip.json`
//!   ([`StripIndex`]) lists every tile's time and frame so a client maps a time to a sprite
//!   and an offset without decoding anything.
//! - **Items:** one per thumbnail (`"0"`, `"1"`, … in `times` order) or per sprite, so a crash
//!   keeps what was done and a retry makes only the rest (`only`).
//! - **Source:** the `1080` proxy the job's manifest lists for the asset
//!   ([`ManifestAsset::proxies`](super::ManifestAsset::proxies): the host lists it and pins it
//!   for the job's life, Decision 12), else the original. Both give the same key (Decision 11): the key names the original's content and
//!   the vendor class of the backend that decodes the original (a proxy's `meta.json` records
//!   it), so a thumbnail's entry does not depend on which source made it.

use fin_core::events::payloads::JobMedia;
use fin_core::{CoreError, Rational};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::invalid;
use super::proxy::no_decoder;
use crate::backend::VendorClass;
use crate::cache::{CacheKey, KeyInput, Recipe, STRIP, THUMBS};
use crate::format::VideoCodec;
use crate::info::Size;

/// The most thumbnails one request makes.
pub const MAX_THUMBS: usize = 64;
/// The longest side of a thumbnail.
pub const MAX_THUMB_SIDE: u32 = 512;
/// The shortest longest-side a thumbnail may ask for.
pub const MIN_THUMB_SIDE: u32 = 2;
/// A filmstrip tile's default height.
pub const DEFAULT_TILE_HEIGHT: u32 = 90;
/// The least and most a filmstrip tile's height may be.
pub const MIN_TILE_HEIGHT: u32 = 16;
pub const MAX_TILE_HEIGHT: u32 = 360;
/// The source seconds one sprite holds.
pub const STRIP_SPRITE_SECONDS: i64 = 10;
/// The source seconds between two tiles.
pub const STRIP_TILE_SECONDS: i64 = 1;
/// The most sprites one filmstrip has (about 11 hours of source).
pub const MAX_STRIP_SPRITES: u32 = 4096;
/// The index of a filmstrip in its entry.
pub const STRIP_INDEX_FILE: &str = "strip.json";

/// What `media.thumbs` makes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ThumbsMode {
    /// A PNG per time (source seconds), its longest side at most `max_side`.
    Thumbs { times: Vec<Rational>, max_side: u32 },
    /// Sprites of tiles `tile_height` tall; `sprites` is [`strip_sprites`] of the source's
    /// duration (the host plans it, the worker checks it: `PLAN_MISMATCH {reason: sprites}`).
    Strip {
        tile_height: u32,
        /// 0 in a plan request: the host fills it from the duration.
        #[serde(default)]
        sprites: u32,
    },
}

impl ThumbsMode {
    /// The cache recipe of the mode.
    pub fn recipe(&self) -> Recipe {
        match self {
            ThumbsMode::Thumbs { .. } => THUMBS,
            ThumbsMode::Strip { .. } => STRIP,
        }
    }

    /// The recipe's key parameters.
    pub fn key_params(&self) -> Value {
        match self {
            ThumbsMode::Thumbs { times, max_side } => {
                json!({ "times": times, "maxSide": max_side })
            }
            ThumbsMode::Strip {
                tile_height,
                sprites,
            } => json!({ "tileHeight": tile_height, "sprites": sprites }),
        }
    }

    /// The job's item ids: `"0"`, `"1"`, … one per thumbnail or sprite.
    pub fn items(&self) -> Vec<String> {
        let n = match self {
            ThumbsMode::Thumbs { times, .. } => times.len(),
            ThumbsMode::Strip { sprites, .. } => *sprites as usize,
        };
        (0..n).map(|i| i.to_string()).collect()
    }

    /// The file of item `i`.
    pub fn file_name(&self, i: usize) -> String {
        match self {
            ThumbsMode::Thumbs { .. } => format!("t{i}.png"),
            ThumbsMode::Strip { .. } => format!("s{i}.png"),
        }
    }
}

/// `media.thumbs` v1: thumbnails or a filmstrip of the asset `asset` (content `fingerprint`),
/// which the cache knows as `key`. A batch kind whose items are the thumbnails or sprites.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThumbsParams {
    pub fingerprint: String,
    /// A project asset, through the job's `in/manifest.json` (its fingerprint there must be
    /// `fingerprint`, else `ASSET_CHANGED`).
    pub asset: String,
    pub mode: ThumbsMode,
    /// The vendor class the host expects the original's decoder to have (part of the key).
    pub decoder_class: VendorClass,
    /// [`thumbs_key`] of the fields above: where the host commits the entry.
    pub key: String,
    /// The items still to make (a retry after a crash: the engine restricts the params to the
    /// items it has not acknowledged); all of them when absent. Files of the others are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub only: Option<Vec<String>>,
    /// The project that asked (host only: `prepare` writes the manifest from its branch head,
    /// `onSucceeded` commits the entry for it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub project_id: Option<String>,
    /// The branch whose head names the asset (`main` when absent). Host only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub branch: Option<String>,
    /// The source's display size, for the kind's memory rule (Decision 18). Host only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Size")]
    pub display: Option<Size>,
}

impl ThumbsParams {
    /// The key input for an original decoded by a backend of `decoder_class`.
    pub fn key_input(&self, decoder_class: VendorClass) -> KeyInput {
        thumbs_key_input(&self.fingerprint, &self.mode, decoder_class)
    }

    /// The items this run makes: `only`, or every item.
    pub fn todo(&self) -> Vec<String> {
        match &self.only {
            Some(only) => only.clone(),
            None => self.mode.items(),
        }
    }

    /// The checks serde cannot make.
    pub fn check(&self) -> Result<(), CoreError> {
        if self.asset.is_empty() || self.asset.len() > 256 {
            return Err(invalid("asset", "1 to 256 characters".into()));
        }
        if self.fingerprint.is_empty() || self.fingerprint.len() > 256 {
            return Err(invalid("fingerprint", "1 to 256 characters".into()));
        }
        match &self.mode {
            ThumbsMode::Thumbs { times, max_side } => {
                if times.is_empty() || times.len() > MAX_THUMBS {
                    return Err(invalid(
                        "mode.times",
                        format!("1 to {MAX_THUMBS} times, not {}", times.len()),
                    ));
                }
                if times.iter().any(Rational::is_negative) {
                    return Err(invalid("mode.times", "a time is not before 0".into()));
                }
                if !(MIN_THUMB_SIDE..=MAX_THUMB_SIDE).contains(max_side) {
                    return Err(invalid(
                        "mode.maxSide",
                        format!("{MIN_THUMB_SIDE} to {MAX_THUMB_SIDE}, not {max_side}"),
                    ));
                }
            }
            ThumbsMode::Strip {
                tile_height,
                sprites,
            } => {
                if !(MIN_TILE_HEIGHT..=MAX_TILE_HEIGHT).contains(tile_height) {
                    return Err(invalid(
                        "mode.tileHeight",
                        format!("{MIN_TILE_HEIGHT} to {MAX_TILE_HEIGHT}, not {tile_height}"),
                    ));
                }
                if !(1..=MAX_STRIP_SPRITES).contains(sprites) {
                    return Err(invalid(
                        "mode.sprites",
                        format!("1 to {MAX_STRIP_SPRITES}, not {sprites}"),
                    ));
                }
            }
        }
        CacheKey::parse(&self.key)
            .map_err(|_| invalid("key", "64 lower-case hex digits".into()))?;
        if let Some(only) = &self.only {
            let all = self.mode.items();
            let mut seen = std::collections::BTreeSet::new();
            if only.iter().any(|i| !all.contains(i) || !seen.insert(i)) {
                return Err(invalid(
                    "only",
                    "each item once, and one of the mode's items".into(),
                ));
            }
        }
        Ok(())
    }
}

/// The key input of thumbnails or a filmstrip (Decision 11).
pub fn thumbs_key_input(
    fingerprint: &str,
    mode: &ThumbsMode,
    decoder_class: VendorClass,
) -> KeyInput {
    KeyInput::new(fingerprint, mode.recipe(), mode.key_params(), decoder_class)
}

/// The cache key of thumbnails or a filmstrip.
pub fn thumbs_key(fingerprint: &str, mode: &ThumbsMode, decoder_class: VendorClass) -> CacheKey {
    CacheKey::compute(&thumbs_key_input(fingerprint, mode, decoder_class))
}

/// The key `media.thumbs` of a `source`-coded original will commit under, with the decoder the
/// capability report names first (the host's half of Decision 11); `CODEC_UNAVAILABLE
/// {direction: decode}` when none decodes it.
pub fn plan_thumbs_key<S: AsRef<str>>(
    fingerprint: &str,
    mode: &ThumbsMode,
    source: VideoCodec,
    backends: &[S],
) -> Result<(CacheKey, &'static crate::backend::BackendEntry), CoreError> {
    let Some(b) = super::proxy::decoder_for(source, backends) else {
        return Err(no_decoder(source));
    };
    Ok((thumbs_key(fingerprint, mode, b.class), b))
}

/// What `mediaThumbsPlan` (fin-node) is asked: the key of a `media.thumbs` job before it is
/// submitted. The decoder class comes from the original (`sourceCodec` and the report's
/// `decoders` for it: the first that decodes it) or, when the original is not read, from the
/// `1080` proxy entry the worker will read (`decoderClass`, which its `meta.json` records:
/// Decision 11 keys the original's decoder class either way). Exactly one of the two.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThumbsPlanRequest {
    pub fingerprint: String,
    /// A filmstrip with `sprites: 0` (or none) is planned from `duration`.
    pub mode: ThumbsMode,
    /// The original's video codec (the asset's `video.codec`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "VideoCodec")]
    pub source_codec: Option<VideoCodec>,
    /// The report's backends for `source_codec`, in the order they are tried.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decoders: Vec<String>,
    /// The original's decoder class as a proxy entry records it (instead of `sourceCodec`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "VendorClass")]
    pub decoder_class: Option<VendorClass>,
    /// The source's duration in seconds (`MediaInfo.duration`; filmstrips).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Rational")]
    pub duration: Option<Rational>,
}

/// What `mediaThumbsPlan` answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThumbsPlan {
    pub key: String,
    /// The backend the report names first for the original's codec (absent when planned from
    /// a proxy's `decoderClass`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub decoder: Option<String>,
    pub decoder_class: VendorClass,
    /// The mode with a filmstrip's sprite count filled in.
    pub mode: ThumbsMode,
    /// The job's items.
    pub items: Vec<String>,
}

/// Plans a `media.thumbs` job ([`ThumbsPlanRequest`]): `INVALID_REQUEST` for a filmstrip
/// without a duration or a request with both or neither decoder sources, `CODEC_UNAVAILABLE`
/// when no listed backend decodes the original's codec.
pub fn plan_thumbs(mut req: ThumbsPlanRequest) -> Result<ThumbsPlan, CoreError> {
    let bad = |why: &str| CoreError::new("INVALID_REQUEST", format!("mediaThumbsPlan: {why}"));
    if let ThumbsMode::Strip { sprites, .. } = &mut req.mode
        && *sprites == 0
    {
        let duration = req
            .duration
            .ok_or_else(|| bad("a filmstrip needs the source's duration"))?;
        *sprites = strip_sprites(duration);
    }
    let (key, decoder, decoder_class) = match (req.source_codec, req.decoder_class) {
        (Some(codec), None) => {
            let (key, b) = plan_thumbs_key(&req.fingerprint, &req.mode, codec, &req.decoders)?;
            (key, Some(b.name.to_string()), b.class)
        }
        (None, Some(class)) if req.decoders.is_empty() => {
            (thumbs_key(&req.fingerprint, &req.mode, class), None, class)
        }
        _ => return Err(bad("give sourceCodec (with decoders) or decoderClass")),
    };
    Ok(ThumbsPlan {
        key: key.to_string(),
        decoder,
        decoder_class,
        items: req.mode.items(),
        mode: req.mode,
    })
}

/// A thumbnail's size (Decision 17): the display size with its longest side at `max_side`,
/// never above the display size itself, each side at least 2.
pub fn thumb_size(display: Size, max_side: u32) -> Size {
    let (w, h) = (display.width.max(1) as u64, display.height.max(1) as u64);
    let longest = w.max(h);
    let cap = (max_side as u64).min(longest);
    let scale = |v: u64| ((2 * v * cap + longest) / (2 * longest)).max(2);
    Size::new(scale(w) as u32, scale(h) as u32)
}

/// A filmstrip tile's size: `tile_height` tall, as wide as the display's aspect ratio gives
/// (rounded to the nearest pixel, at least 2).
pub fn tile_size(display: Size, tile_height: u32) -> Size {
    let (w, h) = (display.width.max(1) as u64, display.height.max(1) as u64);
    let tw = ((2 * w * tile_height as u64 + h) / (2 * h)).max(2);
    Size::new(tw as u32, tile_height)
}

/// The tiles of a source `duration` seconds long: one per second, at least one.
pub fn strip_tiles(duration: Rational) -> u32 {
    if !duration.is_positive() {
        return 1;
    }
    let (n, d) = (duration.num() as i128, duration.den() as i128);
    let secs = STRIP_TILE_SECONDS as i128;
    let tiles = ((n + d * secs - 1) / (d * secs)).max(1);
    tiles.min(u32::MAX as i128) as u32
}

/// The sprites of a source `duration` seconds long: [`STRIP_SPRITE_SECONDS`] of tiles each.
pub fn strip_sprites(duration: Rational) -> u32 {
    let per = (STRIP_SPRITE_SECONDS / STRIP_TILE_SECONDS) as u32;
    strip_tiles(duration).div_ceil(per)
}

/// The source time of tile `j` (counted from the start of the source).
pub fn tile_time(j: u32) -> Rational {
    Rational::from_int(j as i64 * STRIP_TILE_SECONDS)
}

/// What a thumbnail item reports (`item` value): its file in the entry and what it shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThumbItem {
    pub file: String,
    pub width: u32,
    pub height: u32,
    /// The requested time (thumbnails).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Rational")]
    pub at: Option<Rational>,
    /// The original frame index shown (`frame_at`), thumbnails only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub frame: Option<u64>,
    /// The tiles of the sprite (filmstrips).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub tiles: Option<u32>,
}

/// One tile of a sprite in `strip.json`: its source time and the original frame it shows; its
/// x offset in the sprite is its position times the tile width.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StripTile {
    pub at: Rational,
    pub frame: u64,
}

/// One sprite of `strip.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StripSprite {
    pub file: String,
    pub tiles: Vec<StripTile>,
}

/// `strip.json`: where each time of the source is in the sprites.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StripIndex {
    pub version: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    /// [`STRIP_TILE_SECONDS`].
    pub tile_seconds: u32,
    /// [`STRIP_SPRITE_SECONDS`].
    pub sprite_seconds: u32,
    pub sprites: Vec<StripSprite>,
}

/// [`StripIndex::version`].
pub const STRIP_INDEX_VERSION: u32 = 1;

/// What a `media.thumbs` run answers: where its frames came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThumbsRun {
    /// Items made in this run.
    pub made: u32,
    /// `proxy` or `original`.
    pub source: String,
    /// Why the proxy the manifest lists could not be read (the original was read instead):
    /// `missing`, `changed` or `invalid`, as a look's `proxyFallbacks` give it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub proxy_failed: Option<String>,
    /// The entry is complete (`meta.json` written).
    pub complete: bool,
    // The `media` object of the job's `job.done` (6B-1 Task 19): present when diagnostics were
    // on at the worker; numbers and table names only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "JobMedia")]
    pub media: Option<JobMedia>,
}
