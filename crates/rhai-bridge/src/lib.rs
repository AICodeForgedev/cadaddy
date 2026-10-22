//! A single-threaded slot that lets `'static` closures call back into a mutable reference
//! whose lifetime is limited to a synchronous evaluation.
//!
//! # Safety contract
//!
//! 1. [`SlotRef::install`] stores the address of a `&mut T` into a `Cell<*mut T>`.
//! 2. [`SlotRef::clear`] (or the [`SlotGuard`] drop) nulls the pointer.
//! 3. [`SlotRef::with`] dereferences the stored pointer **only while it is installed**.
//!
//! The caller guarantees that:
//! - `install` and `with` are called on the **same thread** (enforced by `!Send + !Sync`).
//! - `clear` is called before the referenced value is dropped (enforced by [`SlotGuard`]).
//! - No other mutable reference to `T` exists while the slot is installed (the caller must
//!   not access the `&mut T` directly between `install` and `clear`).
//!
//! These invariants are upheld by the Rhai evaluation loop in `cadaddy-engine`: the `Session`
//! reference is installed, Rhai evaluates synchronously on the same thread, then the guard
//! clears the slot before the borrow ends.

use std::cell::Cell;
use std::marker::PhantomData;

/// A slot that holds the address of a `&mut T` for the duration of an evaluation.
///
/// Not `Send` or `Sync` — single-threaded only.
pub struct SlotRef<T> {
    ptr: Cell<*mut T>,
    _not_send: PhantomData<*mut T>,
}

impl<T> SlotRef<T> {
    /// Create an empty slot.
    pub const fn new() -> Self {
        Self { ptr: Cell::new(std::ptr::null_mut()), _not_send: PhantomData }
    }

    /// Install a mutable reference.  Returns a guard that clears the slot on drop.
    ///
    /// # Panics
    ///
    /// Panics if a reference is already installed (double-install is a logic error).
    pub fn install<'a>(&'a self, r: &'a mut T) -> SlotGuard<'a, T> {
        assert!(self.ptr.get().is_null(), "SlotRef: already installed");
        self.ptr.set(r as *mut T);
        SlotGuard { slot: self }
    }

    /// Clear the slot.  Safe to call even if already empty.
    pub fn clear(&self) {
        self.ptr.set(std::ptr::null_mut());
    }

    /// Run `f` with a mutable reference to the stored value.
    ///
    /// Returns `None` if the slot is empty (not installed or already cleared).
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let ptr = self.ptr.get();
        if ptr.is_null() {
            return None;
        }
        // SAFETY: The caller of `install` guarantees that:
        // 1. The pointer is valid (it came from a live &mut T).
        // 2. We are on the same thread (SlotRef is !Send).
        // 3. No other mutable reference exists (the &mut T is not used between install and clear).
        // 4. The slot is cleared before the reference is invalidated (SlotGuard drop).
        let r = unsafe { &mut *ptr };
        Some(f(r))
    }

    /// Like `with`, but returns an error if the slot is empty.
    pub fn with_or_err<R, E>(&self, err: E, f: impl FnOnce(&mut T) -> R) -> Result<R, E> {
        self.with(f).ok_or(err)
    }

    /// Install `r`, run `body`, then clear the slot.  Panic-safe: the slot is always cleared.
    ///
    /// This is the primary entry point for evaluating a scripting engine with session access.
    pub fn scoped<R>(&self, r: &mut T, body: impl FnOnce() -> R) -> R {
        let _guard = self.install(r);
        body()
    }
}

impl<T> Default for SlotRef<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Clears the [`SlotRef`] when dropped (panic safety).
pub struct SlotGuard<'a, T> {
    slot: &'a SlotRef<T>,
}

impl<T> Drop for SlotGuard<'_, T> {
    fn drop(&mut self) {
        self.slot.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_read() {
        let slot = SlotRef::new();
        let mut value = 42;
        let _guard = slot.install(&mut value);
        let got = slot.with(|v| *v).unwrap();
        assert_eq!(got, 42);
    }

    #[test]
    fn cleared_after_guard_drop() {
        let slot = SlotRef::new();
        let mut value = 10;
        {
            let _guard = slot.install(&mut value);
            assert!(slot.with(|_| ()).is_some());
        }
        assert!(slot.with(|_| ()).is_none());
    }

    #[test]
    fn empty_returns_none() {
        let slot: SlotRef<i32> = SlotRef::new();
        assert!(slot.with(|_| ()).is_none());
    }

    #[test]
    fn mutate_through_slot() {
        let slot = SlotRef::new();
        let mut value = 0;
        let _guard = slot.install(&mut value);
        slot.with(|v| *v = 99);
        drop(_guard);
        assert_eq!(value, 99);
    }
}
