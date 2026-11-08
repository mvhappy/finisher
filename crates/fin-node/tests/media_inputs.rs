//! The pure parts of a media job's inputs (`src/media/inputs.rs`). The crate is a napi `cdylib`
//! with nothing to link natively, so this includes the napi-free module, as `meta.rs` does.

#![allow(dead_code)]
// Tests build and inspect temporary trees by path; none is a library reader.
#![allow(clippy::disallowed_methods)]

#[path = "../src/media/inputs.rs"]
mod inputs;

mod tests {

    use super::inputs::*;
    use fin_core::Rational;
    use fin_core::eval::{EffectPlan, FramePlan, GraphicPlan, Layer, PlanValue};
    use fin_core::eval::{PlanCrop, PlanTransform, SourceRef};
    use fin_core::model::BlendMode;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::fs;

    fn layer(asset: Option<&str>) -> Layer {
        Layer {
            item: "i".into(),
            kind: fin_core::eval::ItemKind::Clip,
            track: "t".into(),
            local: Rational::zero(),
            source: asset.map(|a| SourceRef {
                asset: a.into(),
                time: Rational::zero(),
            }),
            transform: PlanTransform::default(),
            crop: PlanCrop::default(),
            opacity: 1.0,
            blend: BlendMode::Normal,
            effects: vec![],
            mask: None,
            transition: None,
            caption: None,
            graphic: None,
            nested: None,
            children: None,
        }
    }

    fn plan(layers: Vec<Layer>) -> FramePlan {
        FramePlan {
            timeline: "tl".into(),
            t: Rational::zero(),
            resolution: fin_core::model::Resolution {
                width: 16,
                height: 9,
            },
            layers,
            text_engine: None,
        }
    }

    #[test]
    fn references_cover_sources_effects_graphics_and_children() {
        let mut lut = layer(Some("a1"));
        lut.effects.push(EffectPlan {
            id: "e".into(),
            kind: "lut".into(),
            params: BTreeMap::from([("lut".to_owned(), PlanValue::Asset { asset: "a2".into() })]),
        });
        let mut graphic = layer(None);
        graphic.graphic = Some(GraphicPlan {
            asset: "a3".into(),
            time: Rational::zero(),
            slots: BTreeMap::from([("img".to_owned(), PlanValue::Asset { asset: "a4".into() })]),
        });
        let mut nested = layer(None);
        nested.children = Some(vec![layer(Some("a5"))]);
        let got = referenced_assets(&[
            plan(vec![lut, graphic]),
            plan(vec![nested, layer(Some("a1"))]),
        ]);
        assert_eq!(
            got.into_iter().collect::<Vec<_>>(),
            ["a1", "a2", "a3", "a4", "a5"]
        );
        assert!(referenced_assets(&[]).is_empty());
    }

    #[test]
    fn writes_are_atomic_and_replace_a_link() {
        let dir = tempfile::tempdir().unwrap();
        write_json(dir.path(), "x.json", &json!({ "a": 1 })).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("x.json")).unwrap(),
            r#"{"a":1}"#
        );
        // A leftover temporary and a changed value.
        fs::write(dir.path().join("x.json.tmp"), "junk").unwrap();
        write_json(dir.path(), "x.json", &json!({ "a": 2 })).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("x.json")).unwrap(),
            r#"{"a":2}"#
        );
        assert!(!dir.path().join("x.json.tmp").exists());
        #[cfg(unix)]
        {
            let outside = dir.path().join("outside");
            fs::write(&outside, "keep").unwrap();
            fs::remove_file(dir.path().join("x.json")).unwrap();
            std::os::unix::fs::symlink(&outside, dir.path().join("x.json")).unwrap();
            write_json(dir.path(), "x.json", &json!({ "a": 3 })).unwrap();
            assert_eq!(fs::read_to_string(&outside).unwrap(), "keep");
            assert_eq!(
                fs::read_to_string(dir.path().join("x.json")).unwrap(),
                r#"{"a":3}"#
            );
        }
        let missing = dir.path().join("nope");
        assert_eq!(
            write_json(&missing, "x.json", &json!({})).unwrap_err().code,
            "SHM_REFUSED"
        );
    }

    #[test]
    fn originals_are_regular_files_with_the_openers_facts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.mov");
        fs::write(&file, b"12345").unwrap();
        let o = original_file(&file).unwrap();
        assert_eq!((o.size, o.path.as_str()), (5, file.to_str().unwrap()));
        assert!(o.mtime_ns.parse::<i64>().unwrap() > 0);
        assert!(original_file(&dir.path().join("missing.mov")).is_none());
        assert!(original_file(dir.path()).is_none());
        #[cfg(unix)]
        {
            let link = dir.path().join("link.mov");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(original_file(&link).is_none());
        }
    }
}
