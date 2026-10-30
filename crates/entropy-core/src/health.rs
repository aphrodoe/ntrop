//! Source health tests (stubs; Task 2 implements the SP 800-90B-style rules).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Good,
    Bad,
    Unhealthy,
}

#[derive(Default)]
pub struct TouchHealth;

impl TouchHealth {
    pub fn check(&mut self, dx: i32, dy: i32) -> Verdict {
        if (dx, dy) == (0, 0) {
            Verdict::Bad
        } else {
            Verdict::Good
        }
    }
}

#[derive(Default)]
pub struct CameraHealth;

impl CameraHealth {
    pub fn check(&mut self, _frame: &[u8]) -> Verdict {
        Verdict::Good
    }
}
