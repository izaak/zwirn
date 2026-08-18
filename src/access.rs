//! Internal policy boundary around complete named filesystem accesses.

use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::CoordinatedAccess;

/// The kind of complete filesystem access being coordinated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessKind {
    Read,
    Write,
}

impl fmt::Display for AccessKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => formatter.write_str("read"),
            Self::Write => formatter.write_str("write"),
        }
    }
}

/// Why a macOS coordinated access failed before its filesystem body ran.
#[derive(Debug, Eq, Error, PartialEq)]
pub enum CoordinationFailure {
    #[error("Foundation cannot represent the named filesystem path exactly")]
    PathNotRepresentable,

    #[error("cannot resolve the named path from the current directory: {message}")]
    PathResolution { message: String },

    #[error("coordination failed ({domain} {code}): {message}")]
    Refused {
        domain: String,
        code: i64,
        message: String,
    },

    #[error("Foundation supplied a changed accessor path")]
    AccessorPathChanged,
}

/// One coordinated access that failed before its filesystem body ran.
#[derive(Debug, Eq, PartialEq)]
pub struct CoordinatedAccessFailure {
    kind: AccessKind,
    path: PathBuf,
    reason: CoordinationFailure,
}

impl CoordinatedAccessFailure {
    pub(crate) fn new(kind: AccessKind, path: PathBuf, reason: CoordinationFailure) -> Self {
        Self { kind, path, reason }
    }

    pub fn kind(&self) -> AccessKind {
        self.kind
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn reason(&self) -> &CoordinationFailure {
        &self.reason
    }
}

impl fmt::Display for CoordinatedAccessFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot coordinate {} access to `{}`: {}",
            self.kind,
            self.path.display(),
            self.reason
        )
    }
}

impl Error for CoordinatedAccessFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.reason)
    }
}

/// Statically dispatched access policy for one complete named read or write.
///
/// The outer result reports failure to establish policy access, before `body`
/// runs. The inner result is returned by `body` after access was established.
pub(crate) trait AccessPolicy {
    type Error;

    fn read<T, E>(
        &mut self,
        named_path: &Path,
        body: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, Self::Error>;

    fn write<T, E>(
        &mut self,
        named_path: &Path,
        body: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, Self::Error>;
}

/// Direct, synchronous access selected on non-macOS platforms.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DirectAccess;

impl AccessPolicy for DirectAccess {
    type Error = Infallible;

    fn read<T, E>(
        &mut self,
        _named_path: &Path,
        body: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, Self::Error> {
        Ok(body())
    }

    fn write<T, E>(
        &mut self,
        _named_path: &Path,
        body: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, Self::Error> {
        Ok(body())
    }
}

/// A policy-aware operation outcome.
///
/// Immediately after [`flatten`], `Operation` is the result of a body that ran.
/// Higher layers also use it for ordinary errors outside a policy access.
#[derive(Debug)]
pub(crate) enum PolicyFailure<A, O> {
    Access(A),
    Operation(O),
}

impl<A, O> PolicyFailure<A, O> {
    pub(crate) fn map_access<U>(self, map: impl FnOnce(A) -> U) -> PolicyFailure<U, O> {
        match self {
            Self::Access(error) => PolicyFailure::Access(map(error)),
            Self::Operation(error) => PolicyFailure::Operation(error),
        }
    }

    pub(crate) fn map_operation<U>(self, map: impl FnOnce(O) -> U) -> PolicyFailure<A, U> {
        match self {
            Self::Access(error) => PolicyFailure::Access(error),
            Self::Operation(error) => PolicyFailure::Operation(map(error)),
        }
    }
}

pub(crate) fn flatten<T, A, B>(result: Result<Result<T, B>, A>) -> Result<T, PolicyFailure<A, B>> {
    result
        .map_err(PolicyFailure::Access)?
        .map_err(PolicyFailure::Operation)
}

pub(crate) fn direct_result<T, E>(result: Result<T, PolicyFailure<Infallible, E>>) -> Result<T, E> {
    match result {
        Ok(value) => Ok(value),
        Err(PolicyFailure::Operation(error)) => Err(error),
        Err(PolicyFailure::Access(error)) => match error {},
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[derive(Debug, Eq, PartialEq)]
    struct BodyError(&'static str);

    #[derive(Debug, Eq, PartialEq)]
    struct PolicyError;

    struct RefusingAccess;

    impl AccessPolicy for RefusingAccess {
        type Error = PolicyError;

        fn read<T, E>(
            &mut self,
            _named_path: &Path,
            _body: impl FnOnce() -> Result<T, E>,
        ) -> Result<Result<T, E>, Self::Error> {
            Err(PolicyError)
        }

        fn write<T, E>(
            &mut self,
            _named_path: &Path,
            _body: impl FnOnce() -> Result<T, E>,
        ) -> Result<Result<T, E>, Self::Error> {
            Err(PolicyError)
        }
    }

    #[test]
    fn direct_access_invokes_a_successful_body_once_and_returns_its_value() {
        let calls = Cell::new(0);
        let mut access = DirectAccess;

        let result = access
            .read(Path::new("named"), || {
                calls.set(calls.get() + 1);
                Ok::<_, BodyError>(42)
            })
            .unwrap();

        assert_eq!(calls.get(), 1);
        assert_eq!(result, Ok(42));
    }

    #[test]
    fn direct_access_invokes_a_failing_body_once_and_preserves_its_error() {
        let calls = Cell::new(0);
        let mut access = DirectAccess;

        let result = access
            .write(Path::new("named"), || {
                calls.set(calls.get() + 1);
                Err::<(), _>(BodyError("body failed"))
            })
            .unwrap();

        assert_eq!(calls.get(), 1);
        assert_eq!(result, Err(BodyError("body failed")));
    }

    #[test]
    fn policy_refusal_is_distinct_from_body_failure_and_does_not_invoke_the_body() {
        let calls = Cell::new(0);
        let mut access = RefusingAccess;

        let refused = flatten(access.read(Path::new("named"), || {
            calls.set(calls.get() + 1);
            Err::<(), _>(BodyError("body failed"))
        }));

        assert_eq!(calls.get(), 0);
        assert!(matches!(refused, Err(PolicyFailure::Access(PolicyError))));

        let mut direct = DirectAccess;
        let body_failure = flatten(direct.read(Path::new("named"), || {
            Err::<(), _>(BodyError("body failed"))
        }));
        assert!(matches!(
            body_failure,
            Err(PolicyFailure::Operation(BodyError("body failed")))
        ));
    }
}
