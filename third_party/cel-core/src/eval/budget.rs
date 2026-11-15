//! A budget of evaluation steps (pcx patch, see PATCH.md): every node the
//! evaluator visits spends one step, and every iteration of a
//! comprehension one more; once it is spent, evaluation ends with an
//! error of kind `BudgetExceeded`. Deterministic, as cel-go's cost limit.

use std::sync::atomic::{AtomicU64, Ordering};

/// The steps an evaluation may take.
#[derive(Debug)]
pub struct Budget {
    left: AtomicU64,
}

impl Budget {
    /// A budget of `steps`.
    pub fn new(steps: u64) -> Self {
        Self {
            left: AtomicU64::new(steps),
        }
    }

    /// Spend one step; false when none was left.
    pub fn spend(&self) -> bool {
        let mut left = self.left.load(Ordering::Relaxed);
        loop {
            if left == 0 {
                return false;
            }
            match self.left.compare_exchange_weak(
                left,
                left - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(now) => left = now,
            }
        }
    }

    /// The steps left.
    pub fn left(&self) -> u64 {
        self.left.load(Ordering::Relaxed)
    }
}
