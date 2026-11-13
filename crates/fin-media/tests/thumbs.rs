//! `media.thumbs`' pure parts (plan 6B-1 Task 15, Decision 17): thumbnail and tile sizes, the
//! filmstrip layout, the key and the params' own checks.

use fin_core::Rational;
use fin_media::Size;
use fin_media::backend::VendorClass;
use fin_media::cache::{CacheKey, STRIP, THUMBS};
use fin_media::jobs::{
    DEFAULT_TILE_HEIGHT, MAX_STRIP_SPRITES, MAX_THUMB_SIDE, MAX_THUMBS, STRIP_SPRITE_SECONDS,
    ThumbsMode, ThumbsParams, plan_thumbs_key, strip_sprites, strip_tiles, thumb_size, thumbs_key,
    tile_size,
};
use fin_media::{Rotation, VideoCodec};
use serde_json::json;

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

fn thumbs(times: Vec<Rational>, max_side: u32) -> ThumbsMode {
    ThumbsMode::Thumbs { times, max_side }
}

fn params(mode: ThumbsMode) -> ThumbsParams {
    let key = thumbs_key("fp", &mode, VendorClass::Software).to_string();
    ThumbsParams {
        fingerprint: "fp".into(),
        asset: "a1".into(),
        mode,
        decoder_class: VendorClass::Software,
        key,
        proxy: None,
        only: None,
        project_id: None,
        branch: None,
        display: None,
    }
}

#[test]
fn thumbnails_fit_the_longest_side_and_never_upscale() {
    let t = |w, h, max| {
        let s = thumb_size(Size::new(w, h), max);
        (s.width, s.height)
    };
    assert_eq!(t(1920, 1080, 512), (512, 288));
    assert_eq!(t(1080, 1920, 512), (288, 512));
    // Never above the source.
    assert_eq!(t(320, 180, 512), (320, 180));
    // Square and extreme shapes keep at least 2 pixels.
    assert_eq!(t(1000, 1000, 100), (100, 100));
    assert_eq!(t(4000, 10, 512), (512, 2));
}

#[test]
fn tiles_have_the_asked_height_and_the_displays_aspect() {
    let t = |w, h, th| {
        let s = tile_size(Size::new(w, h), th);
        (s.width, s.height)
    };
    assert_eq!(t(1920, 1080, DEFAULT_TILE_HEIGHT), (160, 90));
    assert_eq!(t(1080, 1920, 90), (51, 90));
    assert_eq!(t(100, 100, 90), (90, 90));
}

#[test]
fn strip_layout_is_one_sprite_per_ten_seconds_and_a_tile_per_second() {
    assert_eq!(STRIP_SPRITE_SECONDS, 10);
    for (secs, tiles, sprites) in [
        (r(1, 30), 1, 1),
        (r(1, 1), 1, 1),
        (r(10, 1), 10, 1),
        (r(101, 10), 11, 2),
        (r(25, 1), 25, 3),
        (r(600, 1), 600, 60),
    ] {
        assert_eq!(strip_tiles(secs), tiles, "{secs}");
        assert_eq!(strip_sprites(secs), sprites, "{secs}");
    }
    // A source of no duration still has one tile.
    assert_eq!(strip_tiles(r(0, 1)), 1);
}

#[test]
fn keys_depend_on_what_is_made_and_nothing_else() {
    let a = thumbs_key(
        "fp",
        &thumbs(vec![r(1, 2), r(3, 1)], 256),
        VendorClass::Software,
    );
    assert_eq!(
        a,
        thumbs_key(
            "fp",
            &thumbs(vec![r(2, 4), r(3, 1)], 256),
            VendorClass::Software
        )
    );
    for other in [
        thumbs_key(
            "fq",
            &thumbs(vec![r(1, 2), r(3, 1)], 256),
            VendorClass::Software,
        ),
        thumbs_key(
            "fp",
            &thumbs(vec![r(3, 1), r(1, 2)], 256),
            VendorClass::Software,
        ),
        thumbs_key(
            "fp",
            &thumbs(vec![r(1, 2), r(3, 1)], 128),
            VendorClass::Software,
        ),
        thumbs_key("fp", &thumbs(vec![r(1, 2), r(3, 1)], 256), VendorClass::Os),
        thumbs_key(
            "fp",
            &ThumbsMode::Strip {
                tile_height: 90,
                sprites: 2,
            },
            VendorClass::Software,
        ),
    ] {
        assert_ne!(a, other);
    }
    assert_eq!(
        thumbs_mode_recipe(&thumbs(vec![r(0, 1)], 64)).name,
        THUMBS.name
    );
    let strip = ThumbsMode::Strip {
        tile_height: 90,
        sprites: 2,
    };
    assert_eq!(thumbs_mode_recipe(&strip).name, STRIP.name);
    assert_ne!(
        thumbs_key("fp", &strip, VendorClass::Software),
        thumbs_key(
            "fp",
            &ThumbsMode::Strip {
                tile_height: 90,
                sprites: 3
            },
            VendorClass::Software
        )
    );
}

fn thumbs_mode_recipe(m: &ThumbsMode) -> fin_media::cache::Recipe {
    m.recipe()
}

#[test]
fn items_are_one_per_thumbnail_or_sprite() {
    assert_eq!(
        thumbs(vec![r(0, 1), r(1, 1), r(2, 1)], 64).items(),
        ["0", "1", "2"]
    );
    assert_eq!(
        ThumbsMode::Strip {
            tile_height: 90,
            sprites: 3
        }
        .items(),
        ["0", "1", "2"]
    );
}

#[test]
fn params_wire_form_and_checks() {
    let p = params(thumbs(vec![r(1, 2)], 256));
    p.check().unwrap();
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(
        v["mode"],
        json!({ "thumbs": { "times": ["1/2"], "maxSide": 256 } })
    );
    let strip = params(ThumbsMode::Strip {
        tile_height: 90,
        sprites: 4,
    });
    assert_eq!(
        serde_json::to_value(&strip).unwrap()["mode"],
        json!({ "strip": { "tileHeight": 90, "sprites": 4 } })
    );
    let back: ThumbsParams = serde_json::from_value(v).unwrap();
    assert_eq!(back, p);

    let bad = |p: ThumbsParams, field: &str| {
        let e = p.check().unwrap_err();
        assert_eq!(e.code, "JOB_PARAMS_INVALID", "{field}");
        assert_eq!(e.details["field"], field, "{field}");
    };
    bad(params(thumbs(vec![], 256)), "mode.times");
    bad(
        params(thumbs(vec![r(0, 1); MAX_THUMBS + 1], 256)),
        "mode.times",
    );
    params(thumbs(vec![r(0, 1); MAX_THUMBS], 256))
        .check()
        .unwrap();
    bad(params(thumbs(vec![r(-1, 1)], 256)), "mode.times");
    bad(
        params(thumbs(vec![r(0, 1)], MAX_THUMB_SIDE + 1)),
        "mode.maxSide",
    );
    bad(params(thumbs(vec![r(0, 1)], 1)), "mode.maxSide");
    bad(
        params(ThumbsMode::Strip {
            tile_height: 8,
            sprites: 1,
        }),
        "mode.tileHeight",
    );
    bad(
        params(ThumbsMode::Strip {
            tile_height: 90,
            sprites: 0,
        }),
        "mode.sprites",
    );
    bad(
        params(ThumbsMode::Strip {
            tile_height: 90,
            sprites: MAX_STRIP_SPRITES + 1,
        }),
        "mode.sprites",
    );
    let mut wrong_key = params(thumbs(vec![r(0, 1)], 64));
    wrong_key.key = "nope".into();
    bad(wrong_key, "key");
    let mut empty_asset = params(thumbs(vec![r(0, 1)], 64));
    empty_asset.asset.clear();
    bad(empty_asset, "asset");
    let mut dup = params(thumbs(vec![r(0, 1)], 64));
    dup.only = Some(vec!["0".into(), "7".into()]);
    bad(dup, "only");
    let mut ok = params(thumbs(vec![r(0, 1), r(1, 1)], 64));
    ok.only = Some(vec!["1".into()]);
    ok.check().unwrap();
    assert_eq!(ok.todo(), ["1"]);
    assert_eq!(
        params(thumbs(vec![r(0, 1), r(1, 1)], 64)).todo(),
        ["0", "1"]
    );
}

#[test]
fn the_host_plans_the_key_from_the_first_decoder_that_reads_the_source() {
    let mode = thumbs(vec![r(1, 1)], 128);
    let (key, b) = plan_thumbs_key("fp", &mode, VideoCodec::Av1, &["dav1d"]).unwrap();
    assert_eq!(b.class, VendorClass::Software);
    assert_eq!(key, thumbs_key("fp", &mode, VendorClass::Software));
    CacheKey::parse(key.as_str()).unwrap();
    let e = plan_thumbs_key("fp", &mode, VideoCodec::H264, &["dav1d"]).unwrap_err();
    assert_eq!(e.code, "CODEC_UNAVAILABLE");
    // Rotation does not enter the key: the source's geometry is part of its fingerprint.
    let _ = Rotation::R90;
}
