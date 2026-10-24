//! Shared pieces for every tool in this repository.
//!
//! Tools are black boxes: callers depend on the JSON contract and on a
//! resource budget, not on how a tool is implemented. This crate owns the
//! budget types and the allocator meter so each tool measures the same way.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

/// Ceiling applied when a request does not name its own budget.
///
/// Debug builds and the first WASM call are much slower than release builds,
/// so the default ceiling is loose enough for a single validation and still
/// rejects a runaway compile. Callers can pass a tighter `budget`.
pub const DEFAULT_MAX_ELAPSED_MICROS: u64 = 5_000_000;
pub const DEFAULT_MAX_PEAK_BYTES: u64 = 128 * 1024 * 1024;

/// Hard cap for the `burn` probe so a request cannot spin the CPU indefinitely.
pub const MAX_BURN_MICROS: u64 = 2_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBudget {
    pub max_elapsed_micros: u64,
    pub max_peak_bytes: u64,
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self {
            max_elapsed_micros: DEFAULT_MAX_ELAPSED_MICROS,
            max_peak_bytes: DEFAULT_MAX_PEAK_BYTES,
        }
    }
}

impl ResourceBudget {
    pub fn merge(elapsed: Option<u64>, peak: Option<u64>) -> Self {
        let defaults = Self::default();
        Self {
            max_elapsed_micros: elapsed.unwrap_or(defaults.max_elapsed_micros),
            max_peak_bytes: peak.unwrap_or(defaults.max_peak_bytes),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceSample {
    pub elapsed_micros: u128,
    pub peak_bytes: u64,
    pub allocated_bytes: u64,
}

impl ResourceSample {
    pub fn within_budget(self, budget: ResourceBudget) -> bool {
        self.elapsed_micros <= u128::from(budget.max_elapsed_micros)
            && self.peak_bytes <= budget.max_peak_bytes
    }
}

struct MeterState {
    enabled: Cell<bool>,
    outstanding: Cell<usize>,
    total: Cell<usize>,
    peak: Cell<usize>,
}

thread_local! {
    static METER: MeterState = const {
        MeterState {
            enabled: Cell::new(false),
            outstanding: Cell::new(0),
            total: Cell::new(0),
            peak: Cell::new(0),
        }
    };
}

fn note_alloc(size: usize) {
    let _ = METER.try_with(|meter| {
        if !meter.enabled.get() {
            return;
        }
        let total = meter.total.get().saturating_add(size);
        let outstanding = meter.outstanding.get().saturating_add(size);
        meter.total.set(total);
        meter.outstanding.set(outstanding);
        if outstanding > meter.peak.get() {
            meter.peak.set(outstanding);
        }
    });
}

fn note_dealloc(size: usize) {
    let _ = METER.try_with(|meter| {
        if !meter.enabled.get() {
            return;
        }
        let outstanding = meter.outstanding.get().saturating_sub(size);
        meter.outstanding.set(outstanding);
    });
}

struct CountingAllocator(System);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.0.alloc(layout) };
        if !ptr.is_null() {
            note_alloc(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.0.alloc_zeroed(layout) };
        if !ptr.is_null() {
            note_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.0.dealloc(ptr, layout) };
        note_dealloc(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { self.0.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size > layout.size() {
                note_alloc(new_size - layout.size());
            } else if layout.size() > new_size {
                note_dealloc(layout.size() - new_size);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator(System);

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "ut")]
unsafe extern "C" {
    /// Microseconds from a host clock. Every language that loads this module
    /// supplies the same import; the module does not call into JavaScript.
    fn now_micros() -> u64;
}

fn monotonic_micros() -> u128 {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { u128::from(now_micros()) }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // A process-local origin keeps the values small and monotonic.
        static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let origin = ORIGIN.get_or_init(Instant::now);
        Instant::now().duration_since(*origin).as_micros()
    }
}

/// Run `body` and report allocations and elapsed time on this thread.
pub fn measure<T>(body: impl FnOnce() -> T) -> (T, ResourceSample) {
    METER.with(|meter| {
        let previous = (
            meter.enabled.get(),
            meter.outstanding.get(),
            meter.total.get(),
            meter.peak.get(),
        );
        meter.outstanding.set(0);
        meter.total.set(0);
        meter.peak.set(0);
        meter.enabled.set(true);
        let started = monotonic_micros();
        let value = body();
        let elapsed = monotonic_micros().saturating_sub(started);
        let sample = ResourceSample {
            elapsed_micros: elapsed,
            peak_bytes: meter.peak.get() as u64,
            allocated_bytes: meter.total.get() as u64,
        };
        meter.enabled.set(previous.0);
        meter.outstanding.set(previous.1);
        meter.total.set(previous.2);
        meter.peak.set(previous.3);
        (value, sample)
    })
}

/// Spin until at least `micros` have elapsed, capped by [`MAX_BURN_MICROS`].
pub fn burn(micros: u64) {
    let micros = u128::from(micros.min(MAX_BURN_MICROS));
    let started = monotonic_micros();
    while monotonic_micros().saturating_sub(started) < micros {
        std::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_observes_a_heap_allocation() {
        let (_, sample) = measure(|| {
            let buffer = vec![0u8; 8 * 1024];
            assert_eq!(buffer.len(), 8 * 1024);
        });
        assert!(
            sample.peak_bytes >= 8 * 1024,
            "peak {} did not cover the buffer",
            sample.peak_bytes
        );
        assert!(sample.allocated_bytes >= 8 * 1024);
    }

    #[test]
    fn budget_rejects_an_over_limit_sample() {
        let sample = ResourceSample {
            elapsed_micros: 50,
            peak_bytes: 100,
            allocated_bytes: 100,
        };
        let budget = ResourceBudget {
            max_elapsed_micros: 10,
            max_peak_bytes: 1_000,
        };
        assert!(!sample.within_budget(budget));
    }

    #[test]
    fn burn_waits_for_the_requested_duration() {
        let (_, sample) = measure(|| burn(20_000));
        assert!(sample.elapsed_micros + 5_000 >= 20_000);
    }
}
