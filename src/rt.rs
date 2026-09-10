//! How a benchmark gets its work onto a runtime, and what that costs.
//!
//! # The problem this exists for
//!
//! Most of this suite drives WorkTable through `futures::executor::block_on`
//! on the calling thread. That polls the future inline: an `await` that pends
//! is woken and re-polled on the same thread, so **no work ever reaches a
//! scheduler**. Run such a benchmark under five pool flavors and it returns
//! five identical numbers, which is worse than no data because it looks like
//! evidence that the flavor does not matter.
//!
//! # The two modes, and what each one actually measures
//!
//! | mode | `WT_BENCH_DISPATCH` | what it is |
//! |---|---|---|
//! | inline | `inline` (default) | `futures::executor::block_on`, polled on the caller's thread |
//! | pool | `pool` | submitted to the selected flavor's pool, caller parks until it finishes |
//!
//! **These are not two ways of measuring the same thing.** Inline measures the
//! operation. Pool measures the operation *plus a submit and a wake*, and on
//! this machine a cross-thread wake is around 2,250 ns while a read that hits
//! a hot page is tens of nanoseconds. So on a micro benchmark the pool mode is
//! mostly measuring the pool.
//!
//! That is the point rather than a flaw. The difference between the two modes
//! is a direct measurement of **what it costs to put one operation on a
//! scheduler**, which is exactly the question behind per-query runtime
//! selection: if a table could name a different runtime for its `update`
//! block than for its `delete` block, this is the price of the hop. Reporting
//! either number without the other, or without saying which mode produced it,
//! would be dishonest, so [`describe`] prints the mode and every result row
//! records it.
//!
//! # Why the `'static` bound
//!
//! Pool mode spawns, and spawning outlives the caller's stack frame as far as
//! the type system is concerned, so the future cannot borrow a local. Call
//! sites that hold a table by reference clone an `Arc` instead. A scoped
//! spawn would avoid that and would need `unsafe` to express, which is not
//! worth it to save an `Arc::clone` in a benchmark.

use std::future::Future;
use std::sync::{Arc, Condvar, Mutex};

/// Where a benchmark's operations are polled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dispatch {
    /// Polled on the calling thread. No scheduler is involved.
    Inline,
    /// Submitted to the selected flavor's pool.
    Pool,
}

impl Dispatch {
    /// The spelling that selects this mode, and what a results row records.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Dispatch::Inline => "inline",
            Dispatch::Pool => "pool",
        }
    }
}

/// The mode this process runs in, read once.
///
/// # Panics
///
/// On an unrecognised value. A benchmark arm that silently fell back to the
/// default is the easiest way to publish a wrong table.
#[must_use]
pub fn dispatch() -> Dispatch {
    static SELECTED: std::sync::OnceLock<Dispatch> = std::sync::OnceLock::new();
    *SELECTED.get_or_init(|| match std::env::var("WT_BENCH_DISPATCH").as_deref() {
        Err(_) | Ok("inline") => Dispatch::Inline,
        Ok("pool") => Dispatch::Pool,
        Ok(other) => panic!("WT_BENCH_DISPATCH={other:?} is not a dispatch mode: expected `inline` or `pool`"),
    })
}

/// One line naming the runtime and the dispatch mode, for stderr and for the
/// results row.
///
/// Both halves matter. `nagoya(spread)` in inline mode is not running spread
/// on anything, because nothing reaches the pool.
#[must_use]
pub fn describe() -> String {
    format!(
        "{} dispatch={}",
        worktable::prelude::describe_tuning(worktable::prelude::engine_flavor()),
        dispatch().name()
    )
}

/// Run `future` to completion, in whichever mode this process selected.
///
/// A drop-in for `futures::executor::block_on` at a call site whose future is
/// `Send + 'static`.
///
/// # Panics
///
/// If the pooled task panics, the panic is observed as a lost result and
/// re-raised here rather than being swallowed on a worker thread.
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match dispatch() {
        Dispatch::Inline => futures::executor::block_on(future),
        Dispatch::Pool => {
            // Parked, not spun. A spinning caller takes a core away from the
            // workers it is waiting on, which would charge the pool for the
            // harness.
            let slot: Arc<(Mutex<Option<F::Output>>, Condvar)> = Arc::new((Mutex::new(None), Condvar::new()));
            let done = Arc::clone(&slot);
            worktable::prelude::engine_executor().spawn(async move {
                let output = future.await;
                let (lock, signal) = &*done;
                *lock.lock().expect("the slot holds no state a panic could corrupt") = Some(output);
                signal.notify_one();
            });
            let (lock, signal) = &*slot;
            let mut held = lock.lock().expect("the slot");
            loop {
                if let Some(output) = held.take() {
                    return output;
                }
                held = signal.wait(held).expect("the wait");
            }
        }
    }
}
