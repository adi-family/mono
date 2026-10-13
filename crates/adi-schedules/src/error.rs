//! The crate's error type: a bad rule (caught once, at the door) and the underlying store.

use std::fmt;

/// The result type every fallible `adi-schedules` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// A failure from [`crate::ScheduleStore`] or the pure slot computation it wraps.
#[derive(Debug)]
pub enum Error {
    /// The rule's `cron` expression does not parse — see [`cron::Schedule`]'s own dialect (`sec
    /// min hour day-of-month month day-of-week [year]`) for what a valid one looks like.
    InvalidCron(String, String),
    /// The rule's `tz` is not a name in the IANA database (`chrono-tz`), e.g. a made-up zone or an
    /// offset (`+02:00`) where a name (`Europe/Berlin`) was wanted.
    UnknownTimezone(String),
    /// Underlying store I/O or SQLite failure.
    Store(anyhow::Error),
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Store(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidCron(expr, detail) => {
                write!(f, "{expr:?} is not a valid cron expression: {detail}")
            }
            Error::UnknownTimezone(tz) => {
                write!(f, "{tz:?} is not an IANA timezone name")
            }
            Error::Store(e) => write!(f, "schedule store error: {e}"),
        }
    }
}

impl std::error::Error for Error {}
