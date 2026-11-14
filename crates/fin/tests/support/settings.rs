//! The offline project the settings, caption and fonts command tests share (plan 4C Task 24).

use serde_json::{Value, json};

/// Rate 30, tl1 1080×1920. Assets: a1 media (60 s), a2 a Lottie template (slot `title` text),
/// a3 a font file (TestSans 400 and 700). t1 video: c1 [0, 5) a1 with effects fx1 blur
/// (radius 4) and fx2 colorAdjust, opacity keyframes at 0 and 1; g1 [5, 8) a2. t2 caption:
/// cap1 [0, 2) "Hello world" style st1, cap2 [2, 4) "Second line" preset:bold. Style st1
/// "Mine" (preset:clean in TestSans 400), pinned from a3.
pub fn snapshot() -> Value {
    let mut st1 =
        serde_json::to_value(fin_core::captions::preset("preset:clean").unwrap()).unwrap();
    st1["id"] = json!("st1");
    st1["name"] = json!("Mine");
    st1["text"]["family"] = json!("TestSans");
    st1["text"]["weight"] = json!(400);
    let face = |w: u16| json!({ "index": if w == 400 { 0 } else { 1 }, "family": "TestSans", "weight": w, "italic": false, "scripts": ["Cyrl", "Latn"] });
    let sha = "ab".repeat(32);
    json!({
        "rev": 9,
        "project": {
            "id": "p1", "name": "Demo", "schemaVersion": 2, "rate": "30",
            "timelines": [{
                "id": "tl1", "name": "Timeline 1", "rate": "30",
                "resolution": { "width": 1080, "height": 1920 },
                "tracks": [
                    { "id": "t1", "kind": "video", "name": "V1", "items": [
                        { "id": "c1", "kind": "clip", "asset": "a1", "start": "0", "sourceIn": "0", "duration": "5",
                          "effects": [
                              { "id": "fx1", "kind": "blur", "params": { "radius": 4.0 } },
                              { "id": "fx2", "kind": "colorAdjust" } ],
                          "keyframes": { "opacity": [
                              { "at": "0", "value": 0.0, "interp": { "type": "linear" } },
                              { "at": "1", "value": 1.0, "interp": { "type": "linear" } } ] } },
                        { "id": "g1", "kind": "graphic", "asset": "a2", "start": "5", "duration": "3" } ] },
                    { "id": "t2", "kind": "caption", "name": "C1", "items": [
                        { "id": "cap1", "kind": "caption", "start": "0", "duration": "2", "style": "st1",
                          "words": [{ "text": "Hello", "start": "0", "end": "1" }, { "text": "world", "start": "1", "end": "2" }] },
                        { "id": "cap2", "kind": "caption", "start": "2", "duration": "2", "style": "preset:bold",
                          "words": [{ "text": "Second", "start": "0", "end": "1" }, { "text": "line", "start": "1", "end": "2" }] } ] },
                ],
            }],
            "assets": {
                "a1": { "id": "a1", "kind": "media", "path": "/m/a1.mp4", "duration": "60", "fingerprint": "a1",
                        "video": { "width": 1920, "height": 1080, "rate": "30" } },
                "a2": { "id": "a2", "kind": "lottieTemplate", "path": "/m/a2.json", "fingerprint": "a2", "duration": "3",
                        "lottie": { "width": 1080, "height": 1920, "rate": "30", "duration": "3",
                                    "slots": [{ "id": "title", "kind": "text" }] } },
                "a3": { "id": "a3", "kind": "font", "path": "/m/a3.ttc", "fingerprint": "a3",
                        "font": { "sha256": sha, "size": 2000, "format": "ttc", "faces": [face(400), face(700)] } },
            },
            "styles": { "st1": st1 },
            "fonts": {
                "pins": { "TestSans|400|normal": {
                    "family": "TestSans", "weight": 400, "italic": false, "face": 0, "sha256": sha,
                    "size": 2000, "format": "ttc", "scripts": ["Cyrl", "Latn"],
                    "source": { "type": "asset", "asset": "a3" } } },
                "fallback": ["TestSans|400|normal"],
            },
        },
    })
}
