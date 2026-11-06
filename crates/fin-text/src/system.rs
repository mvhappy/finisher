//! A `cosmic-text` `FontSystem` built from a request's face list and nothing else.
//!
//! Each face goes into an empty `fontdb::Database` under its own synthetic family name
//! (`fin:<sha256>:<face>`, interned), with the request's weight and a normal style. So every face
//! matches the style exactly, the default family is always the primary face, and the fallback
//! walks the faces in request order. Two faces of one family (or two files claiming one name)
//! can never be confused. The real weight still drives a variable font's `wght` axis.

use std::sync::Arc;

use cosmic_text::fontdb::{self, Language, Source, Stretch, Style, Weight};
use cosmic_text::{Font, FontSystem};

use crate::fallback::FaceListFallback;
use crate::intern::intern;

/// The one locale every system uses (never the platform's).
pub(crate) const LOCALE: &str = "en-US";

/// What a cached system is built from: the request's `(sha256, face)` list, deduplicated in
/// order, and the style weight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Key {
    pub(crate) faces: Vec<(String, u32)>,
    pub(crate) weight: u16,
}

pub(crate) struct Face {
    pub(crate) sha256: String,
    pub(crate) index: u32,
    pub(crate) bytes: Arc<[u8]>,
    pub(crate) id: fontdb::ID,
}

pub(crate) struct System {
    pub(crate) fs: FontSystem,
    /// In request order.
    pub(crate) faces: Vec<Face>,
    pub(crate) primary: &'static str,
}

/// The synthetic family name of one face of one file.
pub(crate) fn face_name(sha256: &str, face: u32) -> &'static str {
    intern(&format!("fin:{sha256}:{face}"))
}

/// Builds the system for `key` from `bytes` (one per face, in order). `Err` names the first
/// face whose bytes do not parse as that face of a font.
pub(crate) fn build(key: &Key, bytes: Vec<Arc<[u8]>>) -> Result<System, String> {
    let mut db = fontdb::Database::new();
    let mut names = Vec::with_capacity(key.faces.len());
    let mut faces = Vec::with_capacity(key.faces.len());
    for ((sha256, index), data) in key.faces.iter().zip(bytes) {
        if cosmic_text::skrifa::FontRef::from_index(&data, *index).is_err() {
            return Err(sha256.clone());
        }
        let name = face_name(sha256, *index);
        let source: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(Arc::clone(&data));
        let id = db.push_face_info(fontdb::FaceInfo {
            id: fontdb::ID::dummy(),
            source: Source::Binary(source),
            index: *index,
            families: vec![(name.to_string(), Language::English_UnitedStates)],
            post_script_name: name.to_string(),
            style: Style::Normal,
            weight: Weight(key.weight),
            stretch: Stretch::Normal,
            monospaced: false,
        });
        names.push(name);
        faces.push(Face {
            sha256: sha256.clone(),
            index: *index,
            bytes: data,
            id,
        });
    }
    for face in &faces {
        if Font::new(&db, face.id, Weight(key.weight)).is_none() {
            return Err(face.sha256.clone());
        }
    }
    let primary = names[0];
    let fallback = FaceListFallback { names };
    let fs = FontSystem::new_with_locale_and_db_and_fallback(LOCALE.to_string(), db, fallback);
    Ok(System { fs, faces, primary })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_database_holds_only_the_request_faces() {
        let sans: Arc<[u8]> = fin_testfonts::test_sans().into();
        let han: Arc<[u8]> = fin_testfonts::test_han().into();
        let key = Key {
            faces: vec![("a".repeat(64), 0), ("b".repeat(64), 0)],
            weight: 400,
        };
        let sys = build(&key, vec![sans, han]).unwrap();
        let families: Vec<&str> = sys
            .fs
            .db()
            .faces()
            .map(|f| f.families[0].0.as_str())
            .collect();
        assert_eq!(
            families,
            [face_name(&"a".repeat(64), 0), face_name(&"b".repeat(64), 0)]
        );
        assert_eq!(sys.fs.locale(), "en-US");
        assert_eq!(sys.primary, families[0]);
    }

    #[test]
    fn unparsable_bytes_name_their_face() {
        let key = Key {
            faces: vec![("c".repeat(64), 0)],
            weight: 400,
        };
        assert_eq!(
            build(&key, vec![Arc::from(&b"not a font"[..])]).err(),
            Some("c".repeat(64))
        );
        let sans: Arc<[u8]> = fin_testfonts::test_sans().into();
        let key = Key {
            faces: vec![("d".repeat(64), 1)],
            weight: 400,
        };
        assert_eq!(build(&key, vec![sans]).err(), Some("d".repeat(64)));
    }
}
