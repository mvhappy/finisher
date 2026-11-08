//! A caption style file (8A Task 12a): `CaptionStyle` JSON without an `id`.

use fin_core::captions::{parse_style_file, presets};
use serde_json::{Value, json};

fn bold() -> Value {
    let mut v = serde_json::to_value(&presets()[0]).unwrap();
    v.as_object_mut().unwrap().remove("id");
    v
}

fn refused(v: &Value) -> String {
    parse_style_file(v.to_string().as_bytes()).expect_err("accepted")
}

#[test]
fn every_preset_without_its_id_is_a_style_file() {
    for p in presets() {
        let mut v = serde_json::to_value(p).unwrap();
        v.as_object_mut().unwrap().remove("id");
        let style = parse_style_file(v.to_string().as_bytes()).unwrap();
        assert_eq!(style.id, "");
        let mut want = p.clone();
        want.id = String::new();
        assert_eq!(style, want, "{}", p.id);
    }
}

#[test]
fn optional_fields_may_be_absent_or_null() {
    let mut v = bold();
    for k in ["stroke", "background", "highlight", "maxDuration"] {
        v.as_object_mut().unwrap().remove(k);
    }
    let style = parse_style_file(v.to_string().as_bytes()).unwrap();
    assert!(style.stroke.is_none() && style.max_duration.is_none());
    v["stroke"] = Value::Null;
    parse_style_file(v.to_string().as_bytes()).unwrap();
}

#[test]
fn shape_errors_are_named() {
    assert!(refused(&json!([1])).contains("JSON object"));
    assert!(parse_style_file(b"{").unwrap_err().contains("not JSON"));
    assert!(
        parse_style_file(&[0xff, 0xfe])
            .unwrap_err()
            .contains("UTF-8")
    );
    let mut v = bold();
    v["id"] = "preset:bold".into();
    assert!(refused(&v).contains("no \"id\""));
    let mut v = bold();
    v.as_object_mut().unwrap().remove("text");
    assert!(refused(&v).contains("not a caption style"));
}

#[test]
fn a_key_the_model_would_drop_is_refused_at_any_depth() {
    let mut v = bold();
    v["colour"] = "#fff".into();
    assert!(refused(&v).contains("colour is not a caption style field"));
    let mut v = bold();
    v["text"]["sizee"] = 3.into();
    assert!(refused(&v).contains("text.sizee is not"));
    let mut v = bold();
    v["stroke"]["wdith"] = 3.into();
    assert!(refused(&v).contains("stroke.wdith is not"));
}

#[test]
fn values_are_checked_as_style_add_checks_them() {
    let cases = [
        ("color", json!("white"), "color"),
        ("safeZone", json!("everywhere"), "safeZone"),
        ("gap", json!("soon"), "soon"),
        ("highlight", json!("#FFD400"), "highlight"),
        ("name", json!(""), "name"),
        ("maxChars", json!(100000), "maxChars"),
    ];
    for (key, value, mention) in cases {
        let mut v = bold();
        v[key] = value;
        let why = refused(&v);
        assert!(why.contains(mention), "{key}: {why}");
    }
    let mut v = bold();
    v["text"]["size"] = 0.into();
    assert!(refused(&v).contains("text.size"));
    let mut v = bold();
    v["position"]["anchor"] = "middle-ish".into();
    assert!(refused(&v).contains("not a caption style"));
}
