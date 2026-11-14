use super::*;
use crate::cache::{EntryFile, RECIPES, recipe};

fn meta(recipe: PcmRecipe, layout: PcmLayout, files: &[(String, u64)]) -> EntryMeta {
    let input = pcm_key_input("fp", recipe, layout, VendorClass::Software);
    EntryMeta {
        key: CacheKey::compute(&input).to_string(),
        fingerprint: input.fingerprint,
        recipe: input.recipe,
        recipe_version: input.recipe_version,
        params: input.params,
        render_version: input.render_version,
        decoder_class: input.decoder_class,
        encoder: None,
        files: files
            .iter()
            .map(|(name, bytes)| EntryFile {
                name: name.clone(),
                bytes: *bytes,
                blake3: "0".repeat(64),
            })
            .collect(),
        media: None,
        tone_mapped: None,
        created_at: 0,
    }
}

#[test]
fn recipes_are_registered() {
    assert_eq!(recipe("pcm48"), Some(PCM48));
    assert_eq!(recipe("pcm16m"), Some(PCM16M));
    assert!(RECIPES.contains(&PCM48) && RECIPES.contains(&PCM16M));
    assert_eq!((PCM48.version, PCM16M.version), (1, 1));
}

#[test]
fn chunk_constants() {
    assert_eq!(PcmLayout::Stereo.chunk_frames(), 131_072);
    assert_eq!(PcmLayout::Mono.chunk_frames(), 262_144);
    assert_eq!(chunk_name(0), "chunk-000000.f32");
    assert_eq!(chunk_name(123_456), "chunk-123456.f32");
    assert_eq!(PcmRecipe::Pcm48.layout(1), PcmLayout::Mono);
    assert_eq!(PcmRecipe::Pcm48.layout(2), PcmLayout::Stereo);
    assert_eq!(PcmRecipe::Pcm48.layout(6), PcmLayout::Stereo);
    assert_eq!(PcmRecipe::Pcm16m.layout(2), PcmLayout::Mono);
}

#[test]
fn key_has_audio_version_and_decoder_class() {
    let input = pcm_key_input(
        "fp",
        PcmRecipe::Pcm48,
        PcmLayout::Stereo,
        VendorClass::Software,
    );
    assert_eq!(
        input.params,
        json!({ "audioVersion": AUDIO_VERSION, "rate": 48000, "layout": "stereo" })
    );
    assert_eq!(input.decoder_class, VendorClass::Software);
    assert_eq!(input.render_version, crate::RENDER_VERSION);
    let base = CacheKey::compute(&input);
    // Another decoder class, layout, recipe or audio version is another key.
    let os = pcm_key("fp", PcmRecipe::Pcm48, PcmLayout::Stereo, VendorClass::Os);
    let mono = pcm_key(
        "fp",
        PcmRecipe::Pcm48,
        PcmLayout::Mono,
        VendorClass::Software,
    );
    let feed = pcm_key(
        "fp",
        PcmRecipe::Pcm16m,
        PcmLayout::Mono,
        VendorClass::Software,
    );
    let mut bumped = input.clone();
    bumped.params["audioVersion"] = json!(AUDIO_VERSION + 1);
    let all = [base.clone(), os, mono, feed, CacheKey::compute(&bumped)];
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert_ne!(a, b);
        }
    }
    assert_eq!(
        pcm_key(
            "fp",
            PcmRecipe::Pcm48,
            PcmLayout::Stereo,
            VendorClass::Software
        ),
        base
    );
}

#[test]
fn plan_key_picks_the_decoder_by_codec() {
    let wav = pcm_plan_key("fp", PcmRecipe::Pcm48, None, 2).unwrap();
    assert_eq!(
        (wav.decoder, wav.decoder_class),
        ("symphonia", VendorClass::Software)
    );
    assert_eq!(wav.layout, PcmLayout::Stereo);
    assert_eq!(
        wav.key,
        pcm_key(
            "fp",
            PcmRecipe::Pcm48,
            PcmLayout::Stereo,
            VendorClass::Software
        )
    );
    let opus = pcm_plan_key("fp", PcmRecipe::Pcm16m, Some(AudioCodec::Opus), 2).unwrap();
    assert_eq!((opus.decoder, opus.layout), ("libopus", PcmLayout::Mono));
    let mono = pcm_plan_key("fp", PcmRecipe::Pcm48, Some(AudioCodec::Flac), 1).unwrap();
    assert_eq!(mono.layout, PcmLayout::Mono);
    let e = pcm_plan_key("fp", PcmRecipe::Pcm48, Some(AudioCodec::Ac3), 2).unwrap_err();
    assert_eq!(e.code, "CODEC_UNAVAILABLE");
    assert_eq!(e.details["direction"], "decode");
    assert_eq!(
        pcm_plan_key("fp", PcmRecipe::Pcm48, None, 0)
            .unwrap_err()
            .code,
        "ASSET_TYPE_UNSUPPORTED"
    );
}

#[test]
fn params_check() {
    let key = pcm_key(
        "fp",
        PcmRecipe::Pcm48,
        PcmLayout::Stereo,
        VendorClass::Software,
    );
    let p = PcmParams {
        fingerprint: "fp".into(),
        recipe: PcmRecipe::Pcm48,
        layout: PcmLayout::Stereo,
        decoder_class: VendorClass::Software,
        key: key.to_string(),
        asset: Some("a1".into()),
        file: None,
        project_id: None,
        branch: None,
    };
    p.check().unwrap();
    let wire = serde_json::to_value(&p).unwrap();
    assert_eq!(wire["recipe"], "pcm48");
    assert_eq!(wire["layout"], "stereo");
    let bad = |f: fn(&mut PcmParams)| {
        let mut q = p.clone();
        f(&mut q);
        q.check().unwrap_err().code
    };
    assert_eq!(bad(|q| q.asset = None), "JOB_PARAMS_INVALID");
    assert_eq!(bad(|q| q.key = "x".into()), "JOB_PARAMS_INVALID");
    assert_eq!(
        bad(|q| {
            q.recipe = PcmRecipe::Pcm16m;
        }),
        "JOB_PARAMS_INVALID"
    );
}

#[test]
fn entry_shape_from_meta() {
    let full = CHUNK_BYTES;
    let files = |sizes: &[u64]| -> Vec<(String, u64)> {
        sizes
            .iter()
            .enumerate()
            .map(|(i, b)| (chunk_name(i), *b))
            .collect()
    };
    // A 10 s stereo source: 3 full chunks and 86,784 frames.
    let m = meta(
        PcmRecipe::Pcm48,
        PcmLayout::Stereo,
        &files(&[full, full, full, 86_784 * 8]),
    );
    m.check().unwrap();
    let e = PcmEntry::of(&m).unwrap();
    assert_eq!(
        (e.frames, e.rate, e.layout),
        (480_000, 48_000, PcmLayout::Stereo)
    );
    assert_eq!(e.chunks.len(), 4);
    // A short chunk before the last, a misnamed chunk, a ragged last chunk: refused.
    for bad in [
        files(&[full, 8, full]),
        vec![("chunk-1.f32".to_string(), 8)],
        files(&[full, 6]),
        files(&[full, 0]),
    ] {
        let m = meta(PcmRecipe::Pcm48, PcmLayout::Stereo, &bad);
        assert_eq!(PcmEntry::of(&m).unwrap_err().code, "PLAN_MISMATCH");
    }
    // Mono chunks hold whole 4-byte frames; another audio version is not this build's entry.
    let m = meta(PcmRecipe::Pcm16m, PcmLayout::Mono, &files(&[full, 4]));
    assert_eq!(PcmEntry::of(&m).unwrap().frames, 262_145);
    let mut old = meta(PcmRecipe::Pcm16m, PcmLayout::Mono, &files(&[4]));
    old.params["audioVersion"] = json!(0);
    assert_eq!(PcmEntry::of(&old).unwrap_err().code, "PLAN_MISMATCH");
}
