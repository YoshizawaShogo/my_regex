#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    UnexpectedEof,
    UnexpectedToken(char),
    UnbalancedParen,
    UnbalancedClass,
    EmptyClass,
    BadRange(char, char),
    DanglingQuantifier,
}

#[derive(Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub pos: usize,
}

pub(crate) fn err<T>(kind: ErrorKind, pos: usize) -> Result<T, Error> {
    Err(Error { kind, pos })
}
