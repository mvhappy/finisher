//! The media cache's pure parts (6B-1 plan Task 13, Decisions 10–11): recipes, the key over
//! canonical JSON and `meta.json`'s checks.

use fin_media::RENDER_VERSION;
use fin_media::backend::VendorClass;
use fin_media::cache::{
    CacheKey, EntryFile, EntryMeta, KeyInput, PROXY_540, PROXY_1080, RECIPES, Recipe, STRIP,
    THUMBS, canonical_json, recipe,
};
use serde_json::json;

fn input() -> KeyInput {
    KeyInput::new(
        "b3:0123",
        PROXY_1080,
        json!({ "tier": "t1080", "codec": "h264" }),
        VendorClass::Os,
    )
}

fn meta(files: &[(&str, &[u8])]) -> EntryMeta {
    let i = input();
    EntryMeta {
        key: CacheKey::compute(&i).to_string(),
        fingerprint: i.fingerprint,
        recipe: i.recipe,
        recipe_version: i.recipe_version,
        params: i.params,
        render_version: i.render_version,
        decoder_class: i.decoder_class,
        encoder: None,
        files: files
            .iter()
            .map(|(name, bytes)| EntryFile {
                name: (*name).to_string(),
                bytes: bytes.len() as u64,
                blake3: blake3::hash(bytes).to_hex().to_string(),
            })
            .collect(),
        media: None,
        tone_mapped: None,
        created_at: 1,
    }
}

#[test]
fn recipes_are_named_and_versioned() {
    let names: Vec<&str> = RECIPES.iter().map(|r| r.name).collect();
    assert_eq!(
        names,
        [
            "proxy-540",
            "proxy-1080",
            "thumbs",
            "strip",
            "pcm48",
            "pcm16m",
            "wave"
        ]
    );
    assert_eq!(recipe("proxy-540"), Some(PROXY_540));
    assert_eq!(recipe("thumbs"), Some(THUMBS));
    assert_eq!(recipe("strip"), Some(STRIP));
    assert_eq!(recipe("proxy-360"), None);
    for r in RECIPES {
        assert!(r.version >= 1);
        assert!(Recipe::valid_name(r.name));
    }
    // A later tier (a phone-sized proxy, the mobile note) is another name of the same shape.
    assert!(Recipe::valid_name("proxy-360"));
    assert!(!Recipe::valid_name("Proxy 360"));
    assert!(!Recipe::valid_name(""));
}

#[test]
fn canonical_json_sorts_keys_and_drops_whitespace() {
    let a = json!({ "b": 1, "a": { "y": [true, null, "x\"y"], "x": -2 } });
    assert_eq!(
        canonical_json(&a),
        r#"{"a":{"x":-2,"y":[true,null,"x\"y"]},"b":1}"#
    );
}

#[test]
fn key_is_blake3_of_the_canonical_input() {
    let i = input();
    let text = canonical_json(&json!({
        "decoderClass": "os",
        "fingerprint": "b3:0123",
        "params": { "codec": "h264", "tier": "t1080" },
        "recipe": "proxy-1080",
        "recipeVersion": 1,
        "renderVersion": RENDER_VERSION,
    }));
    let want = blake3::hash(text.as_bytes()).to_hex().to_string();
    let key = CacheKey::compute(&i);
    assert_eq!(key.as_str(), want);
    assert_eq!(key.shard(), &want[..2]);
    // Field order in params does not matter; every field does.
    let mut reordered = i.clone();
    reordered.params = json!({ "codec": "h264", "tier": "t1080" });
    assert_eq!(CacheKey::compute(&reordered), key);
    for change in [
        KeyInput {
            fingerprint: "b3:0124".into(),
            ..i.clone()
        },
        KeyInput {
            recipe: "proxy-540".into(),
            ..i.clone()
        },
        KeyInput {
            recipe_version: 2,
            ..i.clone()
        },
        KeyInput {
            params: json!({ "tier": "t1080", "codec": "av1" }),
            ..i.clone()
        },
        KeyInput {
            render_version: RENDER_VERSION + 1,
            ..i.clone()
        },
        KeyInput {
            decoder_class: VendorClass::Gpu,
            ..i.clone()
        },
    ] {
        assert_ne!(CacheKey::compute(&change), key, "{change:?}");
    }
}

#[test]
fn key_parse_accepts_only_lowercase_hex() {
    let key = CacheKey::compute(&input());
    assert_eq!(CacheKey::parse(key.as_str()).unwrap(), key);
    assert!(CacheKey::parse(&key.as_str().to_uppercase()).is_err());
    assert!(CacheKey::parse("abc").is_err());
    assert!(CacheKey::parse(&format!("{}/", &key.as_str()[..63])).is_err());
    assert!(CacheKey::parse("../x").is_err());
}

#[test]
fn meta_check_recomputes_the_key() {
    let m = meta(&[("proxy.mp4", b"abc"), ("table.bin", b"t")]);
    m.check().unwrap();
    assert_eq!(m.bytes().unwrap(), 4);
    // Sizes that overflow a u64 are refused, by `bytes` and by `check`.
    let mut huge = m.clone();
    for f in &mut huge.files {
        f.bytes = u64::MAX / 2 + 1;
    }
    let e = huge.bytes().unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["reason"]),
        ("PLAN_MISMATCH", &json!("cacheEntry"))
    );
    assert_eq!(huge.check().unwrap_err().code, "PLAN_MISMATCH");
    assert_eq!(m.key_input(), input());
    let mut forged = m.clone();
    forged.params = json!({ "tier": "t540" });
    let e = forged.check().unwrap_err();
    assert_eq!(e.code, "PLAN_MISMATCH");
    assert_eq!(e.details["reason"], "cacheKey");
}

#[test]
fn meta_check_refuses_bad_file_lists() {
    let bad = [
        vec![("meta.json", &b"x"[..])],
        vec![("../x", &b"x"[..])],
        vec![("a/b", &b"x"[..])],
        vec![(".hidden", &b"x"[..])],
        vec![("", &b"x"[..])],
        vec![("a", &b"x"[..]), ("a", &b"y"[..])],
    ];
    for files in bad {
        let e = meta(&files).check().unwrap_err();
        assert_eq!(e.code, "PLAN_MISMATCH", "{files:?}");
        assert_eq!(e.details["reason"], "cacheEntry");
    }
    let mut m = meta(&[("a", b"x")]);
    m.files[0].blake3 = "zz".into();
    assert!(m.check().is_err());
    let mut m = meta(&[("a", b"x")]);
    m.render_version = 0;
    m.key = CacheKey::compute(&m.key_input()).to_string();
    m.check().unwrap();
}

#[test]
fn meta_json_is_camel_case_and_strict() {
    let m = meta(&[("a", b"x")]);
    let v = serde_json::to_value(&m).unwrap();
    for k in [
        "key",
        "fingerprint",
        "recipe",
        "recipeVersion",
        "params",
        "renderVersion",
        "decoderClass",
        "files",
        "createdAt",
    ] {
        assert!(v.get(k).is_some(), "{k}");
    }
    assert!(v.get("encoder").is_none());
    let mut extra = v.clone();
    extra["surprise"] = json!(1);
    assert!(serde_json::from_value::<EntryMeta>(extra).is_err());
    let back: EntryMeta = serde_json::from_value(v).unwrap();
    assert_eq!(back, m);
}
