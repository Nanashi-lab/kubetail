use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Error reading data")]
    IOError(#[from] std::io::Error),
}
