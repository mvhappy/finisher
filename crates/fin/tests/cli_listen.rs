//! `fin listen` against a real finisherd and its `fin-workers media` (plan 6C Task 21): a WAV
//! tone on an audio track, measured for the mix and per track (`--json` and the summary lines),
//! the range resolved and echoed, a range past the end refused, and `--no-wait`.

mod support;

use std::path::Path;

use serde_json::Value;
use support::{Server, assert_error, fin, ok};

/// A 16-bit stereo WAV at 48 kHz of `secs` of a 1 kHz tone at peak `peak`.
fn wav(path: &Path, secs: usize, peak: f64) {
    let frames = secs * 48_000;
    let mut data = Vec::with_capacity(frames * 4);
    for i in 0..frames {
        let v = (peak * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0).sin() * 32767.0)
            .round() as i16;
        data.extend_from_slice(&v.to_le_bytes());
        data.extend_from_slice(&v.to_le_bytes());
    }
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&48_000u32.to_le_bytes());
    h.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    h.extend_from_slice(&4u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(data.len() as u32).to_le_bytes());
    h.extend_from_slice(&data);
    std::fs::write(path, h).unwrap();
}

#[test]
fn fin_listen_text_and_json() {
    let Some(server) = Server::start_with("fin_listen_text_and_json", Vec::new(), None) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    ok(fin(
        sd,
        root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = root.join("film");
    wav(&dir.join("tone.wav"), 4, 0.1414);
    let run = |args: &[&str]| ok(fin(sd, &dir, args));
    run(&["asset", "add", "tone.wav"]);
    run(&[
        "clip", "insert", "--track", "t2", "--at", "0", "--asset", "a1", "--dur", "3s",
    ]);

    // The whole timeline, as JSON: the mix and the track, a silence-free 3 s tone.
    let doc = run(&["listen", "--json"]);
    assert_eq!(
        (
            doc["timeline"].as_str(),
            doc["from"].as_str(),
            doc["to"].as_str()
        ),
        (Some("tl1"), Some("0"), Some("3")),
        "{doc}"
    );
    let i = doc["mix"]["integrated"].as_f64().unwrap();
    // A 1 kHz stereo tone at −20 dBFS RMS a channel: −16.99 LUFS, within 0.3 LU (16-bit).
    assert!((i - -16.99).abs() <= 0.3, "{doc}");
    assert_eq!(doc["tracks"][0]["track"], "t2", "{doc}");
    assert_eq!(doc["mix"]["clipped"]["samples"], 0);
    assert_eq!(doc["mix"]["silences"], Value::Array(vec![]));
    assert_eq!(
        doc["stats"]["sourcesDecoded"].as_u64().unwrap()
            + doc["stats"]["sourcesFromCache"].as_u64().unwrap(),
        1,
        "{doc}"
    );

    // A range in the timeline's own times, and the summary lines.
    let doc = run(&["listen", "--from", "1s", "--to", "f75"]);
    assert_eq!(
        (doc["from"].as_str(), doc["to"].as_str()),
        (Some("1"), Some("5/2")),
        "{doc}"
    );
    let lines: Vec<&str> = doc["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "{doc}");
    assert!(
        lines[0].starts_with("mix: -17.0 LUFS integrated") || lines[0].starts_with("mix: -16."),
        "{doc}"
    );
    assert!(lines[1].starts_with("track t2: "), "{doc}");

    // Past the end, and an empty range.
    assert_error(
        &fin(sd, &dir, &["listen", "--to", "10s"]),
        3,
        "INVALID_RANGE",
    );
    assert_error(
        &fin(sd, &dir, &["listen", "--from", "2s", "--to", "2s"]),
        3,
        "INVALID_RANGE",
    );

    // `--no-wait` answers the job; waiting for it gives the result.
    let doc = run(&["listen", "--no-wait"]);
    let job = doc["job"]["id"].as_str().unwrap().to_string();
    let waited = run(&["wait", &job]);
    assert!(waited.to_string().contains("integrated"), "{waited}");
}
