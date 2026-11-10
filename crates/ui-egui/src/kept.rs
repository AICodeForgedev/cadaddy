//! What a palette or dialog keeps between frames in egui's memory, moved out for the frame and
//! back after it, never copied (Q6482).
//!
//! `get_temp` clones what it reads: a palette holding a command's answer about the whole model
//! (the Model Tree, a job's findings, a sheet list) copied it and dropped the old copy on every
//! frame, ~7 ms a frame on a hotel-size model in the Coordination palette, more on a slow laptop.
//! [`take`] removes the value from egui's memory and [`put`] puts it back, both moves.
//!
//! Between the two nothing else may read the value: a palette takes its own state once at the top
//! of its frame and puts it back at the end. Should the frame end early (a panic the fault guard
//! catches), the palette starts afresh next frame, as it would after its tab is reopened.

/// The value kept under `id`, moved out of egui's memory (`None`: nothing kept yet).
pub fn take<T: 'static + Clone + Default + Send + Sync>(ctx: &egui::Context, id: egui::Id) -> Option<T> {
    ctx.data_mut(|d| d.remove_temp::<T>(id))
}

/// Keep `value` under `id` for the next frame.
pub fn put<T: 'static + Clone + Send + Sync>(ctx: &egui::Context, id: egui::Id, value: T) {
    ctx.data_mut(|d| d.insert_temp(id, value));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CLONES: AtomicUsize = AtomicUsize::new(0);

    #[derive(Default)]
    struct Counted(Arc<()>);

    impl Clone for Counted {
        fn clone(&self) -> Self {
            CLONES.fetch_add(1, Ordering::Relaxed);
            Self(Arc::clone(&self.0))
        }
    }

    #[test]
    fn a_kept_value_is_moved_not_copied() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("kept-test");
        assert!(take::<Counted>(&ctx, id).is_none());
        put(&ctx, id, Counted::default());
        for _ in 0..10 {
            let v = take::<Counted>(&ctx, id).unwrap_or_default();
            put(&ctx, id, v);
        }
        assert_eq!(CLONES.load(Ordering::Relaxed), 0, "ten frames, no copy");
        // Taken means gone until it is put back.
        let v = take::<Counted>(&ctx, id);
        assert!(v.is_some());
        assert!(take::<Counted>(&ctx, id).is_none());
    }
}
