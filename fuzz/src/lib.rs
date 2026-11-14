//! What every fuzz target shares: [`run`], which keeps the panics CADaddy catches itself from
//! counting as crashes.
//!
//! Some readers run code that may panic under `catch_unwind` on purpose and report it as a read
//! error (the DWG bridge runs its unmodified reader that way, crates/dwg). libfuzzer-sys installs
//! a panic hook that aborts on every panic, caught or not, so each of those stopped the target as
//! a crash before CADaddy's own `catch_unwind` was reached: a false alarm that also kept the
//! fuzzer from finding real hangs, OOMs and stack overflows. [`run`] replaces that hook with one
//! that only remembers the panic; a panic that escapes CADaddy is printed here and then still
//! aborts, through libfuzzer-sys's own `catch_unwind`.

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Mutex, Once};

static QUIET: Once = Once::new();

/// The last panic's message, place and (with `RUST_BACKTRACE`) backtrace, from any thread.
static LAST: Mutex<String> = Mutex::new(String::new());

fn last() -> std::sync::MutexGuard<'static, String> {
    LAST.lock().unwrap_or_else(|e| e.into_inner())
}

/// Installs the hook that remembers panics instead of aborting on them, once per process.
fn quiet_panics() {
    QUIET.call_once(|| {
        panic::set_hook(Box::new(|info| {
            let bt = std::backtrace::Backtrace::capture();
            *last() = format!("{info}\n{bt}");
        }))
    });
}

/// Runs one input. A panic that `f` catches itself is not a crash; one that escapes `f` is
/// returned with what the hook saw of it.
fn guarded(f: impl FnOnce()) -> Result<(), (String, Box<dyn std::any::Any + Send>)> {
    quiet_panics();
    last().clear();
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|p| (std::mem::take(&mut *last()), p))
}

/// Runs one fuzz input: a panic CADaddy catches is a read failure, a panic that escapes it is
/// printed and passed on, so libfuzzer-sys still aborts and saves the input.
pub fn run(f: impl FnOnce()) {
    if let Err((seen, payload)) = guarded(f) {
        eprintln!("panic escaped CADaddy: {seen}");
        panic::resume_unwind(payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// One test, because the panic hook is per process.
    #[test]
    fn caught_panics_are_not_crashes_escaped_ones_are() {
        // libfuzzer-sys's hook counts as a crash (it aborts) every panic it sees.
        static CRASHES: AtomicUsize = AtomicUsize::new(0);
        panic::set_hook(Box::new(|_| {
            CRASHES.fetch_add(1, Ordering::SeqCst);
        }));

        // A reader that catches its own panic and reports a read error, as the DWG bridge does.
        let reader = |bytes: &[u8]| -> Result<usize, String> {
            panic::catch_unwind(|| {
                let mut offset = usize::MAX - 1;
                for b in bytes {
                    offset = offset.checked_add(usize::from(*b)).unwrap_or_else(|| panic!("attempt to add with overflow"));
                }
                offset
            })
            .map_err(|_| "the reader failed on this file".to_string())
        };
        let mut read = None;
        run(|| read = Some(reader(&[7, 9])));
        assert_eq!(read, Some(Err("the reader failed on this file".to_string())));
        assert_eq!(CRASHES.load(Ordering::SeqCst), 0, "a panic the reader caught was reported as a crash");

        // A panic nothing in CADaddy catches still reaches the harness, with its message.
        let escaped = guarded(|| panic!("escaped at offset {}", 42));
        let (seen, payload) = escaped.err().unwrap_or_else(|| panic!("an escaped panic was swallowed"));
        assert!(seen.contains("escaped at offset 42"), "{seen}");
        let said = payload.downcast_ref::<&str>().copied().or_else(|| payload.downcast_ref::<String>().map(String::as_str));
        assert_eq!(said, Some("escaped at offset 42"));
        assert!(panic::catch_unwind(|| run(|| panic!("again"))).is_err());

        // A panic on another thread that the reader joins and reports is not a crash either.
        let mut joined = None;
        run(|| joined = Some(std::thread::spawn(|| panic!("worker")).join().is_err()));
        assert_eq!(joined, Some(true));
        assert_eq!(CRASHES.load(Ordering::SeqCst), 0);
        let _ = panic::take_hook();
    }
}
