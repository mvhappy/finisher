//! The one process-wide family-name interner (4C Decision 38). `cosmic-text` fallback lists
//! hold `&'static str`, so each distinct name is leaked exactly once and reused after.

use std::collections::BTreeSet;
use std::sync::Mutex;

static NAMES: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

/// The interned copy of `name`; the first call for a name leaks it, later calls reuse it.
pub fn intern(name: &str) -> &'static str {
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = names.get(name) {
        return found;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

/// The interned copy of `name`, if it was ever interned (leaks nothing).
pub fn interned(name: &str) -> Option<&'static str> {
    NAMES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(name)
        .copied()
}

/// How many distinct names have been leaked so far in this process.
pub fn interned_count() -> usize {
    NAMES.lock().unwrap_or_else(|e| e.into_inner()).len()
}
