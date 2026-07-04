use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The spec parsed as JSON but is semantically invalid or unsupported.
    Spec(String),
    /// The data cannot support the requested encoding.
    Data(String),
}

impl Error {
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Spec(_) => "spec",
            Error::Data(_) => "data",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Spec(m) | Error::Data(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}
