//! [`ApproxBytes`]: a payload's serialised size, estimated without serialising (Decision 3).
//! The journal's in-flight byte bound counts it in the caller. Strings and vectors count
//! their lengths, scalars and enums a fixed width, structs their field names and punctuation
//! ([`approx_struct!`]). An estimate, within 2× of the serialised size on the samples
//! (`approx_bytes_bounds_serialised_size`). An edit intent is the exception: its exact size
//! is computed once when it is built ([`super::intent`]).

use std::collections::BTreeMap;

use serde_json::Value;

use super::class::{
    Blake3Hex, BlobHash, BlobRef, BranchId, Code, Content, Hashed, IdRef, KindName, Name, PathRef,
    ProjectId, Scrubbed, SrcLoc, Ulid, Version,
};

/// An estimate of the serialised JSON size, in bytes.
pub trait ApproxBytes {
    fn approx_bytes(&self) -> usize;
}

/// Implements [`ApproxBytes`] for a struct: each field's estimate plus its name and
/// punctuation, plus the braces.
macro_rules! approx_struct {
    ($ty:ty { $($field:ident),* $(,)? }) => {
        impl $crate::events::approx::ApproxBytes for $ty {
            fn approx_bytes(&self) -> usize {
                2 $(+ stringify!($field).len() + 4
                    + $crate::events::approx::ApproxBytes::approx_bytes(&self.$field))*
            }
        }
    };
}
pub(crate) use approx_struct;

/// Implements [`ApproxBytes`] as a fixed width (enums serialised as short strings, scalars).
macro_rules! approx_fixed {
    ($n:expr => $($ty:ty),* $(,)?) => {$(
        impl $crate::events::approx::ApproxBytes for $ty {
            fn approx_bytes(&self) -> usize {
                $n
            }
        }
    )*};
}
pub(crate) use approx_fixed;

approx_fixed!(1 => ());
approx_fixed!(5 => bool);
approx_fixed!(4 => u8, u16, u32, i32);
approx_fixed!(8 => u64, i64, f64, usize);

impl ApproxBytes for str {
    fn approx_bytes(&self) -> usize {
        self.len() + 2
    }
}

impl ApproxBytes for String {
    fn approx_bytes(&self) -> usize {
        self.as_str().approx_bytes()
    }
}

macro_rules! approx_as_str {
    ($($ty:ty),*) => {$(
        impl ApproxBytes for $ty {
            fn approx_bytes(&self) -> usize {
                self.as_str().len() + 2
            }
        }
    )*};
}
approx_as_str!(
    Name, Code, Hashed, IdRef, BlobHash, ProjectId, BranchId, Version, SrcLoc, Scrubbed, Ulid,
    KindName, Blake3Hex
);

impl<T: ApproxBytes + ?Sized> ApproxBytes for &T {
    fn approx_bytes(&self) -> usize {
        (**self).approx_bytes()
    }
}

impl<T: ApproxBytes + ?Sized> ApproxBytes for Box<T> {
    fn approx_bytes(&self) -> usize {
        (**self).approx_bytes()
    }
}

impl<T: ApproxBytes> ApproxBytes for Option<T> {
    fn approx_bytes(&self) -> usize {
        self.as_ref().map_or(0, ApproxBytes::approx_bytes)
    }
}

impl<T: ApproxBytes> ApproxBytes for Vec<T> {
    fn approx_bytes(&self) -> usize {
        2 + self.iter().map(|x| x.approx_bytes() + 1).sum::<usize>()
    }
}

impl<K: ApproxBytes, V: ApproxBytes> ApproxBytes for BTreeMap<K, V> {
    fn approx_bytes(&self) -> usize {
        2 + self
            .iter()
            .map(|(k, v)| k.approx_bytes() + v.approx_bytes() + 2)
            .sum::<usize>()
    }
}

impl<T: ApproxBytes> ApproxBytes for Content<T> {
    fn approx_bytes(&self) -> usize {
        self.0.approx_bytes()
    }
}

impl ApproxBytes for Value {
    fn approx_bytes(&self) -> usize {
        match self {
            Value::Null => 4,
            Value::Bool(_) => 5,
            Value::Number(_) => 8,
            Value::String(s) => s.approx_bytes(),
            Value::Array(a) => 2 + a.iter().map(|x| x.approx_bytes() + 1).sum::<usize>(),
            Value::Object(m) => {
                2 + m
                    .iter()
                    .map(|(k, v)| k.approx_bytes() + v.approx_bytes() + 2)
                    .sum::<usize>()
            }
        }
    }
}

/// The bytes a [`BlobRef`] takes in its entry (not the blob's own bytes, which the journal
/// counts when it queues them).
impl ApproxBytes for BlobRef {
    fn approx_bytes(&self) -> usize {
        40 + self.hash.approx_bytes() + self.media_type.approx_bytes()
    }
}

impl ApproxBytes for PathRef {
    fn approx_bytes(&self) -> usize {
        36 + self.ext.approx_bytes() + self.hash.approx_bytes()
    }
}
