//! The child's three pipe ends are created non-inheritable (`std::io::pipe`) and made
//! inheritable **only on the spawn thread, immediately around `CreateProcessW`** (Decision
//! 10), where `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` names them as the only handles the child
//! inherits. [`Inheritable`] flips them on and, when dropped, off again, whatever happened in
//! between.

use std::io;

use windows_sys::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};

use super::check;

/// While alive, `handles` are inheritable.
pub(super) struct Inheritable<'a> {
    handles: &'a [HANDLE],
}

fn set(h: HANDLE, on: bool) -> io::Result<()> {
    let flags = if on { HANDLE_FLAG_INHERIT } else { 0 };
    // SAFETY: a valid handle we own; only its inherit flag changes.
    check(unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, flags) })
}

impl<'a> Inheritable<'a> {
    pub fn new(handles: &'a [HANDLE]) -> io::Result<Inheritable<'a>> {
        for (i, &h) in handles.iter().enumerate() {
            if let Err(e) = set(h, true) {
                for &done in &handles[..i] {
                    let _ = set(done, false);
                }
                return Err(e);
            }
        }
        Ok(Inheritable { handles })
    }
}

impl Drop for Inheritable<'_> {
    fn drop(&mut self) {
        for &h in self.handles {
            let _ = set(h, false);
        }
    }
}

/// Whether `h` is inheritable (tests).
#[cfg(test)]
pub(super) fn is_inheritable(h: HANDLE) -> bool {
    use windows_sys::Win32::Foundation::GetHandleInformation;
    let mut flags = 0;
    // SAFETY: a valid handle; `flags` is a valid out-pointer.
    unsafe { GetHandleInformation(h, &mut flags) != 0 && flags & HANDLE_FLAG_INHERIT != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::raw;

    #[test]
    fn pipes_start_non_inheritable_and_flip_back() {
        let (r, w) = std::io::pipe().unwrap();
        let hs = [raw(&r), raw(&w)];
        assert!(!is_inheritable(hs[0]) && !is_inheritable(hs[1]));
        {
            let _on = Inheritable::new(&hs).unwrap();
            assert!(is_inheritable(hs[0]) && is_inheritable(hs[1]));
        }
        assert!(!is_inheritable(hs[0]) && !is_inheritable(hs[1]));
    }
}
