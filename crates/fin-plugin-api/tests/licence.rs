//! Licence classes (8A Decision 6).

use fin_plugin_api::{LicenceClass, classify};

#[test]
fn licence_classes() {
    use LicenceClass::*;
    for (spdx, want) in [
        ("MIT", Permissive),
        ("Apache-2.0", Permissive),
        ("MIT OR Apache-2.0", Permissive),
        ("Apache-2.0 WITH LLVM-exception", Permissive),
        ("BSD-3-Clause AND ISC", Permissive),
        ("OFL-1.1", Permissive),
        ("CC0-1.0", Permissive),
        ("MPL-2.0", WeakCopyleft),
        ("EPL-2.0", WeakCopyleft),
        ("MIT AND MPL-2.0", WeakCopyleft),
        ("MPL-2.0 OR MIT", Permissive),
        ("GPL-3.0-only", Copyleft),
        ("GPL-2.0-or-later", Copyleft),
        ("GPL-2.0+", Copyleft),
        ("GPL-2.0", Copyleft),
        ("LGPL-2.1-only", Copyleft),
        ("AGPL-3.0-or-later", Copyleft),
        ("SSPL-1.0", Copyleft),
        ("EUPL-1.2", Copyleft),
        ("CC-BY-SA-4.0", Copyleft),
        ("GPL-2.0-only WITH Classpath-exception-2.0", Copyleft),
        ("MIT OR GPL-3.0-only", Permissive),
        ("MIT AND GPL-3.0-only", Copyleft),
        ("(MIT OR GPL-3.0-only) AND LGPL-2.1-only", Copyleft),
        ("CC-BY-NC-4.0", NonCommercial),
        ("CC-BY-NC-SA-4.0", NonCommercial),
        ("CC-BY-NC-ND-4.0", NonCommercial),
        ("PolyForm-Noncommercial-1.0.0", NonCommercial),
        ("LicenseRef-Acme-NonCommercial", NonCommercial),
        ("MIT AND CC-BY-NC-4.0", NonCommercial),
        ("MIT OR CC-BY-NC-4.0", Permissive),
        ("LicenseRef-acme", Unknown),
        ("Artistic-2.0", Unknown),
        ("MIT WITH Classpath-exception-2.0", Unknown),
        ("not a licence", Unknown),
        ("", Unknown),
        ("mit", Unknown),
        ("MIT AND LicenseRef-acme", Unknown),
        ("MIT OR LicenseRef-acme", Permissive),
        ("GPL-3.0-only AND LicenseRef-acme", Copyleft),
        ("non-commercial use only", NonCommercial),
    ] {
        assert_eq!(classify(spdx), want, "{spdx:?}");
    }
}
