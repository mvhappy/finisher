//! Our own fallback list (4C Decision 38): exactly the request's faces, in request order.
//! `PlatformFallback` is never constructed, so no platform font name can ever be consulted.

use cosmic_text::Fallback;
use unicode_script::Script;

pub(crate) struct FaceListFallback {
    /// The interned family names of the request's faces, primary first.
    pub(crate) names: Vec<&'static str>,
}

impl Fallback for FaceListFallback {
    fn common_fallback(&self) -> &[&'static str] {
        &self.names
    }

    fn forbidden_fallback(&self) -> &[&'static str] {
        &[]
    }

    fn script_fallback(&self, _script: Script, _locale: &str) -> &[&'static str] {
        &[]
    }
}
