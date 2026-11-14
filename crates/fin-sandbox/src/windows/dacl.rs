//! Whether a DACL already gives a grant's access (plan 7C Task 10 review B1). Pure, so it is
//! tested on every OS; [`super::acl::admits`] reads a real DACL into [`Ace`]s.
//!
//! The toolchain roots (`C:\Windows\System32`, `%ProgramFiles%\Git`) already admit every
//! AppContainer through `ALL APPLICATION PACKAGES` (`S-1-15-2-1`), and a non-elevated user may
//! not change their DACL (no `WRITE_DAC`). So the broker treats a read grant whose access the
//! DACL already gives as done, without a change, and reads a static grant back before it
//! skips it. Counted: allow ACEs of `ALL APPLICATION PACKAGES` (explicit or inherited; Finisher
//! never revokes them) and *explicit* allow ACEs of the grant's own SID (an inherited one goes
//! with its parent's grant). Conservative: any deny ACE of either for part of the mask makes
//! it not given, and an inheritable grant needs the access inherited by both files and
//! directories (no `NO_PROPAGATE_INHERIT`).

/// `ALL APPLICATION PACKAGES`.
pub const ALL_APPLICATION_PACKAGES: &str = "S-1-15-2-1";

/// `OBJECT_INHERIT_ACE`.
pub const OBJECT_INHERIT: u8 = 0x01;
/// `CONTAINER_INHERIT_ACE`.
pub const CONTAINER_INHERIT: u8 = 0x02;
/// `NO_PROPAGATE_INHERIT_ACE`.
pub const NO_PROPAGATE: u8 = 0x04;
/// `INHERIT_ONLY_ACE`.
pub const INHERIT_ONLY: u8 = 0x08;
/// `INHERITED_ACE`.
pub const INHERITED: u8 = 0x10;

/// Whom an ACE names, as far as this check cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    AllPackages,
    Grantee,
    Other,
}

/// One ACE of a file's DACL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ace {
    /// Allow (`ACCESS_ALLOWED_ACE_TYPE`), else deny (`ACCESS_DENIED_ACE_TYPE`).
    pub allow: bool,
    pub flags: u8,
    pub who: Who,
    pub mask: u32,
}

/// `mask` with the generic rights mapped to the file rights (`FILE_GENERIC_*`).
pub fn map_generic(mask: u32) -> u32 {
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const GENERIC_EXECUTE: u32 = 0x2000_0000;
    const GENERIC_ALL: u32 = 0x1000_0000;
    let mut m = mask & 0x0FFF_FFFF;
    for (g, f) in [
        (GENERIC_READ, 0x0012_0089),
        (GENERIC_WRITE, 0x0012_0116),
        (GENERIC_EXECUTE, 0x0012_00A0),
        (GENERIC_ALL, 0x001F_01FF),
    ] {
        if mask & g != 0 {
            m |= f;
        }
    }
    m
}

/// Whether `aces` give the grantee `mask` on the object, and to its files and directories
/// when `inherit`.
pub fn covers(aces: &[Ace], mask: u32, inherit: bool) -> bool {
    let counted = |a: &&Ace| match a.who {
        Who::AllPackages => true,
        Who::Grantee => a.flags & INHERITED == 0,
        Who::Other => false,
    };
    let denied = aces
        .iter()
        .filter(|a| !a.allow && a.who != Who::Other)
        .any(|a| map_generic(a.mask) & mask != 0);
    if denied {
        return false;
    }
    let allowed = |keep: &dyn Fn(u8) -> bool| {
        aces.iter()
            .filter(|a| a.allow)
            .filter(counted)
            .filter(|a| keep(a.flags))
            .fold(0, |m, a| m | map_generic(a.mask))
    };
    let own = allowed(&|f| f & INHERIT_ONLY == 0);
    if own & mask != mask {
        return false;
    }
    if !inherit {
        return true;
    }
    let to = |bit: u8| allowed(&move |f| f & bit != 0 && f & NO_PROPAGATE == 0);
    to(OBJECT_INHERIT) & mask == mask && to(CONTAINER_INHERIT) & mask == mask
}

#[cfg(test)]
mod tests {
    use super::*;

    const RX: u32 = 0x0012_00A9;
    const GR_GX: u32 = 0xA000_0000;

    fn ace(who: Who, flags: u8, mask: u32) -> Ace {
        Ace {
            allow: true,
            flags,
            who,
            mask,
        }
    }

    #[test]
    fn system32s_dacl_admits_every_container() {
        // `(A;;0x1200a9;;;AC)(A;OICIIO;GXGR;;;AC)`, as System32 carries it.
        let aces = [
            ace(Who::Other, 0, 0x001F_01FF),
            ace(Who::AllPackages, 0, RX),
            ace(
                Who::AllPackages,
                OBJECT_INHERIT | CONTAINER_INHERIT | INHERIT_ONLY,
                GR_GX,
            ),
        ];
        assert!(covers(&aces, RX, true));
        assert!(!covers(&aces, 0x0013_01BF, false), "not a write");
    }

    #[test]
    fn an_inherited_all_packages_ace_counts_a_grantees_does_not() {
        let inherited = OBJECT_INHERIT | CONTAINER_INHERIT | INHERITED;
        assert!(covers(&[ace(Who::AllPackages, inherited, RX)], RX, true));
        assert!(!covers(&[ace(Who::Grantee, inherited, RX)], RX, true));
        assert!(covers(
            &[ace(Who::Grantee, OBJECT_INHERIT | CONTAINER_INHERIT, RX)],
            RX,
            true
        ));
    }

    #[test]
    fn what_is_not_given_is_not_covered() {
        // Nothing for containers (a user's profile, a drive root).
        assert!(!covers(&[ace(Who::Other, 0, 0x001F_01FF)], RX, false));
        // Inherited by the subtree only (inherit-only), not on the object itself.
        let io = OBJECT_INHERIT | CONTAINER_INHERIT | INHERIT_ONLY;
        assert!(!covers(&[ace(Who::AllPackages, io, RX)], RX, false));
        // On the object only, not inherited.
        assert!(!covers(&[ace(Who::AllPackages, 0, RX)], RX, true));
        assert!(covers(&[ace(Who::AllPackages, 0, RX)], RX, false));
        // Inherited by files only, or one level only.
        assert!(!covers(
            &[ace(Who::AllPackages, OBJECT_INHERIT, RX)],
            RX,
            true
        ));
        assert!(!covers(
            &[ace(
                Who::AllPackages,
                OBJECT_INHERIT | CONTAINER_INHERIT | NO_PROPAGATE,
                RX
            )],
            RX,
            true
        ));
        // Read only, execute asked.
        assert!(!covers(&[ace(Who::AllPackages, 0, 0x0012_0089)], RX, false));
        // A deny of part of it.
        let deny = Ace {
            allow: false,
            flags: 0,
            who: Who::AllPackages,
            mask: 0x20,
        };
        assert!(!covers(&[deny, ace(Who::AllPackages, 0, RX)], RX, false));
    }
}
