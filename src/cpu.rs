//! User plus system CPU across every thread of this process.
//!
//! **The column that turns a misdiagnosis around.** Wall time alone cannot
//! tell a runtime that finished quickly from one that finished quickly by
//! burning eight cores to do it, and it cannot tell slow code from idle cores.
//! Low CPU with low throughput is a scheduler leaving cores unused; high CPU
//! with low throughput is the code being slow. Those want opposite fixes, and
//! a table without this column cannot distinguish them.
//!
//! Reported as `cpu_x`, which is `(user + system) / real`: the average number
//! of cores busy over the run.

/// Seconds of user plus system CPU consumed by this process so far.
#[must_use]
pub fn cpu_seconds() -> f64 {
    #[repr(C)]
    #[derive(Default)]
    struct Timeval {
        sec: i64,
        usec: i32,
        _pad: i32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Rusage {
        utime: Timeval,
        stime: Timeval,
        rest: [i64; 14],
    }
    unsafe extern "C" {
        fn getrusage(who: i32, usage: *mut Rusage) -> i32;
    }
    let mut usage = Rusage::default();
    // SAFETY: `who` is RUSAGE_SELF and the struct is the layout the platform
    // writes; the trailing fields are only ever read as opaque words.
    unsafe {
        getrusage(0, &raw mut usage);
    }
    usage.utime.sec as f64
        + f64::from(usage.utime.usec) / 1e6
        + usage.stime.sec as f64
        + f64::from(usage.stime.usec) / 1e6
}
