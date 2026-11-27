//! How a command ended.

/// How a command ended. The caller turns it into an exit status; errors in
/// the command line itself are the caller's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Done, possibly with warnings.
    Success,
    /// Done, but there were warnings and the caller treats them as failure.
    Warnings,
    /// Stopped before anything was written.
    Stopped,
    /// An error left the command unable to decide or finish.
    Failed,
    /// The output failed verification, or is not ready to swap in.
    NotReady,
    Interrupted,
}

impl Outcome {
    /// A command that finished: `strict` makes warnings a failure.
    pub const fn finished(warned: bool, strict: bool) -> Self {
        if warned && strict {
            Outcome::Warnings
        } else {
            Outcome::Success
        }
    }
}
