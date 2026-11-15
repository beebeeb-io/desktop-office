//! A test instrument, not part of the API: how many input bytes this crate has handed to a reader
//! (WordCraft, GridCraft, DeckCraft) on the current thread. A test reads it to prove that a
//! refusal came before any reader saw the file, so nothing was inflated. Counting costs one add
//! per open; nothing is recorded but the number.

use std::cell::Cell;

thread_local! {
    static TO_READERS: Cell<u64> = const { Cell::new(0) };
}

/// Count `bytes` as handed to a reader. Called right before every reader call in this crate.
pub(crate) fn handed(bytes: &[u8]) {
    TO_READERS.with(|n| n.set(n.get().saturating_add(bytes.len() as u64)));
}

/// Bytes handed to readers on this thread since the last [`reset`].
#[doc(hidden)]
pub fn bytes_to_readers() -> u64 {
    TO_READERS.with(Cell::get)
}

/// Start counting from 0 on this thread.
#[doc(hidden)]
pub fn reset() {
    TO_READERS.with(|n| n.set(0));
}
