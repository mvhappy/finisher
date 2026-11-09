//! The pairing kind (5B plan Decision 33): `auth.pair` v1, metadata only, with closed
//! platform and route tables. The emitter is finisherd's `auth/pairing.ts`.

use std::collections::BTreeSet;

use fin_core::events::*;
use serde_json::json;

#[test]
fn auth_pair_payload_is_closed() {
    let k = kind("auth.pair", 1).expect("auth.pair at v1");
    assert!(!k.reserved && !k.training_only);
    assert_eq!(k.sources, [Source::Finisherd]);
    let schema = (k.schema)();
    let props: BTreeSet<&str> = schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .map(String::as_str)
        .collect();
    // Metadata only (5B Decision 33): no name, address, id, safety code or secret.
    assert_eq!(
        props,
        BTreeSet::from(["outcome", "platform", "route", "micros"]),
        "{schema}"
    );
    for ok in [
        json!({"outcome": "offered"}),
        json!({"outcome": "approved", "platform": "android", "route": "tailscale", "micros": 5}),
        json!({"outcome": "expired"}),
    ] {
        assert!(decode_payload(k, &ok).is_ok(), "{ok}");
    }
    for bad in [
        json!({"outcome": "stolen"}),
        json!({"outcome": "redeemed", "platform": "amiga"}),
        json!({"outcome": "redeemed", "route": "127.0.0.1"}),
    ] {
        assert!(decode_payload(k, &bad).is_err(), "{bad}");
    }
}
