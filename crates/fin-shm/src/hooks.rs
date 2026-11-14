//! Test hooks (feature `test-hooks`): count the `mmap` calls this thread made, force the copy
//! path of [`crate::commit_output`], and run a callback between its checks and its rename
//! (to swap the file under it). Per thread: each call runs on its caller's thread, so parallel
//! tests never see each other's hooks. Without the feature every hook is inert.

#[cfg(feature = "test-hooks")]
mod imp {
    use std::cell::RefCell;

    #[derive(Default)]
    struct State {
        maps: u64,
        force_copy: bool,
        before_rename: Option<Box<dyn FnMut()>>,
    }

    thread_local! { static STATE: RefCell<State> = RefCell::default(); }

    /// The `mmap` calls this thread made so far (writers' and sealed readers').
    pub fn mmap_calls() -> u64 {
        STATE.with(|s| s.borrow().maps)
    }

    /// From now on, on this thread, `commit_output` copies as if the volumes differed.
    pub fn force_copy(on: bool) {
        STATE.with(|s| s.borrow_mut().force_copy = on);
    }

    /// Runs `f` once, in the next `commit_output` on this thread, after its checks and before
    /// its rename or copy.
    pub fn before_rename(f: impl FnMut() + 'static) {
        STATE.with(|s| s.borrow_mut().before_rename = Some(Box::new(f)));
    }

    pub(crate) fn count_map() {
        STATE.with(|s| s.borrow_mut().maps += 1);
    }

    pub(crate) fn copy_forced() -> bool {
        STATE.with(|s| s.borrow().force_copy)
    }

    pub(crate) fn run_before_rename() {
        let f = STATE.with(|s| s.borrow_mut().before_rename.take());
        if let Some(mut f) = f {
            f();
        }
    }
}

#[cfg(not(feature = "test-hooks"))]
mod imp {
    pub(crate) fn count_map() {}

    pub(crate) fn copy_forced() -> bool {
        false
    }

    pub(crate) fn run_before_rename() {}
}

#[cfg(feature = "test-hooks")]
pub use imp::*;
#[cfg(not(feature = "test-hooks"))]
pub(crate) use imp::*;
