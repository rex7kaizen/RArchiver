use std::fmt;
use std::io;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Io(String),
    NotFound(String),
    PermissionDenied(String),
    Exists(String),
    InvalidName(String),
    Invalid(String),
    Cancelled,
    NeedPassword,
    WrongPassword,
    Unsupported(String),
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(s) => write!(f, "G/Ç hatası: {s}"),
            Error::NotFound(s) => write!(f, "Bulunamadı: {s}"),
            Error::PermissionDenied(s) => write!(f, "İzin yok: {s}"),
            Error::Exists(s) => write!(f, "Zaten var: {s}"),
            Error::InvalidName(s) => write!(f, "Geçersiz ad: {s}"),
            Error::Invalid(s) => write!(f, "{s}"),
            Error::Cancelled => write!(f, "İptal edildi"),
            Error::NeedPassword => write!(f, "Parola gerekli"),
            Error::WrongPassword => write!(f, "Parola yanlış"),
            Error::Unsupported(s) => write!(f, "Desteklenmiyor: {s}"),
            Error::Corrupt(s) => write!(f, "Arşiv bozuk: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound => Error::NotFound(e.to_string()),
            io::ErrorKind::PermissionDenied => Error::PermissionDenied(e.to_string()),
            io::ErrorKind::AlreadyExists => Error::Exists(e.to_string()),
            io::ErrorKind::Interrupted if e.to_string() == "cancelled" => Error::Cancelled,
            _ => Error::Io(e.to_string()),
        }
    }
}
