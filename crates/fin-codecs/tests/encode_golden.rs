//! A small encoded-output golden for cross-machine comparison (6A-1 review). **Same-machine
//! only:** SVT-AV1 is byte-deterministic with one thread on one machine, but its C code is
//! compiled with the platform's floating-point contraction (arm64 clang uses FMA), so other
//! machine classes may produce other bytes. The test prints how this machine compares with the
//! recorded entry for its class and asserts only with `FIN_ENCODE_GOLDEN=1`;
//! `FIN_ENCODE_GOLDEN=record` prints the entry to add.

use fin_codecs::fixture::{ClipSpec, encode_all, fixture_config};
use fin_codecs::{EncoderConfig, ProbeResult, RawFrame, open_encoder};
use fin_container::CodecConfig;
use fin_core::Rational;
use fin_media::backend::EncodePurpose;
use fin_media::testing::pattern;
use fin_media::{Size, VideoCodec};
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/encode.json");

/// 20 pattern frames (seed 9) at 320×180, preset 10, CRF 35, keyint 15, one thread: the
/// av1C record then every packet, BLAKE3 and byte count.
fn encode() -> (String, usize, usize) {
    let spec = ClipSpec {
        size: Size::new(320, 180),
        rate: Rational::from_int(30),
        preset: 10,
        crf: 35,
        keyint: 15,
        ..ClipSpec::default()
    };
    let mut enc = open_encoder(
        &EncoderConfig {
            purpose: EncodePurpose::TestFixture,
            ..fixture_config(&spec, VideoCodec::Av1)
        },
        &ProbeResult::software_only(),
    )
    .unwrap();
    let packets = encode_all(&mut enc, 20, |i| {
        let p = pattern(i, 320, 180, 9);
        RawFrame::i420(spec.size, p.y, p.u, p.v)
    })
    .unwrap();
    let CodecConfig::Av1 { av1c } = enc.config().unwrap() else {
        panic!("AV1")
    };
    let mut bytes = av1c;
    for p in &packets {
        bytes.extend_from_slice(&p.data);
    }
    (
        blake3::hash(&bytes).to_hex().to_string(),
        bytes.len(),
        packets.len(),
    )
}

#[test]
fn encoded_output_against_this_machine_class() {
    let golden: Value = serde_json::from_str(GOLDEN).unwrap();
    let class = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let (hash, bytes, packets) = encode();
    assert_eq!(packets, 20, "one temporal unit a frame");
    let entry = golden["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["class"] == class.as_str());
    let mode = std::env::var("FIN_ENCODE_GOLDEN").unwrap_or_default();
    if mode == "record" {
        eprintln!(r#"{{ "class": "{class}", "blake3": "{hash}", "bytes": {bytes} }}"#);
        return;
    }
    match entry {
        Some(e) if e["blake3"] == hash.as_str() && e["bytes"] == bytes => {
            eprintln!("encode golden: {class} matches")
        }
        Some(e) => {
            eprintln!(
                "encode golden: {class} differs (recorded {} / {} bytes, got {hash} / {bytes})",
                e["blake3"], e["bytes"]
            );
            assert!(
                mode != "1",
                "the encoded output changed on this machine class"
            );
        }
        None => {
            eprintln!("encode golden: no entry for {class} (got {hash} / {bytes})");
            assert!(mode != "1", "no recorded entry for {class}");
        }
    }
}
