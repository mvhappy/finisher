//! What an acquisition replaced ([`Previous`]), for the owner's ordering rule (module docs
//! of [`super`]).

/// What an acquisition replaced: the owner needs it for the ordering rule (module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Previous {
    /// The server id of the `lease.json` record that was replaced (`None`: it was missing or
    /// damaged).
    pub record: Option<String>,
    /// `lease.json` was damaged (its writer is unknown).
    pub damaged: bool,
    /// The claim files below the new epoch seen while acquiring, with their owners (`None`:
    /// unreadable).
    pub claims: Vec<(u64, Option<String>)>,
}

impl Previous {
    /// Whether another server (than `server_id`) may have written to the folder up to this
    /// acquisition, after epoch `known`: the replaced record names another server or is
    /// damaged, or a claim above `known` belongs to another server (or to nobody readable).
    pub fn other_writer_since(&self, server_id: &str, known: u64) -> bool {
        self.damaged
            || self.record.as_deref().is_some_and(|s| s != server_id)
            || self
                .claims
                .iter()
                .any(|(e, owner)| *e > known && owner.as_deref() != Some(server_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_writer_is_the_record_or_a_newer_foreign_claim() {
        let p = |record: Option<&str>, damaged, claims: Vec<(u64, Option<&str>)>| Previous {
            record: record.map(str::to_string),
            damaged,
            claims: claims
                .into_iter()
                .map(|(e, o)| (e, o.map(str::to_string)))
                .collect(),
        };
        assert!(!p(None, false, vec![]).other_writer_since("me", 0), "free");
        assert!(!p(Some("me"), false, vec![(1, Some("me"))]).other_writer_since("me", 0));
        assert!(p(Some("you"), false, vec![]).other_writer_since("me", 9));
        assert!(p(None, true, vec![]).other_writer_since("me", 9), "damaged");
        assert!(p(Some("me"), false, vec![(3, Some("you"))]).other_writer_since("me", 2));
        assert!(!p(Some("me"), false, vec![(3, Some("you"))]).other_writer_since("me", 3));
        assert!(
            p(Some("me"), false, vec![(3, None)]).other_writer_since("me", 2),
            "unreadable"
        );
    }
}
