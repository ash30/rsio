use std::fmt;

pub type Result<T> = std::result::Result<T,SessionError>;

#[derive(Debug)]
pub enum SessionError {
    UnknownSession,
    SessionClosed,
    MultipleInflightPollRequest,
    SessionUnresponsive,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self)
    }
}

