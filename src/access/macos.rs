//! Synchronous Rust wrapper for the Objective-C file-coordination bridge.

use std::borrow::Cow;
use std::ffi::{c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::Path;
use std::thread::Result as ThreadResult;

use super::{AccessKind, AccessPolicy, CoordinatedAccessFailure, CoordinationFailure};

const ACCESS_PATH_NOT_REPRESENTABLE: i32 = 1;
const ACCESS_COORDINATION_FAILED: i32 = 2;
const ACCESSOR_PATH_CHANGED: i32 = 3;
const ACCESS_INTERNAL_FAILURE: i32 = 4;

const ACCESS_READ: i32 = 0;
const ACCESS_WRITE: i32 = 1;

/// Short-lived `NSFileCoordinator` claims for complete named accesses.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CoordinatedAccess;

impl AccessPolicy for CoordinatedAccess {
    type Error = CoordinatedAccessFailure;

    fn read<R>(&mut self, named_path: &Path, body: impl FnOnce() -> R) -> Result<R, Self::Error> {
        coordinated(named_path, AccessKind::Read, body)
    }

    fn write<R>(&mut self, named_path: &Path, body: impl FnOnce() -> R) -> Result<R, Self::Error> {
        coordinated(named_path, AccessKind::Write, body)
    }
}

fn coordinated<R, F>(
    named_path: &Path,
    kind: AccessKind,
    body: F,
) -> Result<R, CoordinatedAccessFailure>
where
    F: FnOnce() -> R,
{
    let claimed_path = absolute_claim_path(named_path)
        .map_err(|reason| CoordinatedAccessFailure::new(kind, named_path.to_path_buf(), reason))?;
    let bytes = claimed_path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.contains(&0) {
        return Err(CoordinatedAccessFailure::new(
            kind,
            named_path.to_path_buf(),
            CoordinationFailure::PathNotRepresentable,
        ));
    }

    let mut state = BodyState {
        body: Some(body),
        completion: None,
    };
    let mut outcome = NativeOutcome::new();
    // SAFETY: the byte slice, callback state, and outcome remain alive until
    // this synchronous native call returns. The trampoline catches unwinds.
    unsafe {
        zwirn_coordinated_access(
            bytes.as_ptr(),
            bytes.len(),
            match kind {
                AccessKind::Read => ACCESS_READ,
                AccessKind::Write => ACCESS_WRITE,
            },
            Some(invoke_body::<R, F>),
            (&mut state as *mut BodyState<R, F>).cast(),
            &mut outcome,
        );
    }

    finish_coordinated(named_path, kind, state, outcome)
}

fn finish_coordinated<R, F>(
    named_path: &Path,
    kind: AccessKind,
    state: BodyState<R, F>,
    outcome: NativeOutcome,
) -> Result<R, CoordinatedAccessFailure> {
    if let Some(completion) = state.completion {
        return match completion {
            Ok(value) => Ok(value),
            Err(panic) => resume_unwind(panic),
        };
    }
    if outcome.status == ACCESS_INTERNAL_FAILURE {
        panic_internal_failure(&outcome);
    }

    let reason = match outcome.status {
        ACCESS_PATH_NOT_REPRESENTABLE => CoordinationFailure::PathNotRepresentable,
        ACCESS_COORDINATION_FAILED => CoordinationFailure::Refused {
            domain: outcome.domain(),
            code: outcome.native_code,
            message: outcome.message(),
        },
        ACCESSOR_PATH_CHANGED => CoordinationFailure::AccessorPathChanged,
        status => panic!("coordination bridge returned unknown status {status}"),
    };
    Err(CoordinatedAccessFailure::new(
        kind,
        named_path.to_path_buf(),
        reason,
    ))
}

fn absolute_claim_path(path: &Path) -> Result<Cow<'_, Path>, CoordinationFailure> {
    if path.is_absolute() {
        Ok(Cow::Borrowed(path))
    } else {
        std::env::current_dir()
            .map(|cwd| Cow::Owned(cwd.join(path)))
            .map_err(|error| CoordinationFailure::PathResolution {
                message: error.to_string(),
            })
    }
}

struct BodyState<R, F> {
    body: Option<F>,
    completion: Option<ThreadResult<R>>,
}

unsafe extern "C" fn invoke_body<R, F>(context: *mut c_void)
where
    F: FnOnce() -> R,
{
    if context.is_null() {
        return;
    }
    // SAFETY: `coordinated` supplies this exact state to a synchronous call.
    let state = unsafe { &mut *context.cast::<BodyState<R, F>>() };
    let Some(body) = state.body.take() else {
        return;
    };
    state.completion = Some(catch_unwind(AssertUnwindSafe(body)));
}

#[repr(C)]
struct NativeOutcome {
    status: i32,
    native_code: i64,
    native_domain: [c_char; 128],
    message: [c_char; 1024],
}

impl NativeOutcome {
    const fn new() -> Self {
        Self {
            status: ACCESS_INTERNAL_FAILURE,
            native_code: 0,
            native_domain: [0; 128],
            message: [0; 1024],
        }
    }

    fn domain(&self) -> String {
        bounded_string(&self.native_domain)
    }

    fn message(&self) -> String {
        bounded_string(&self.message)
    }

    fn message_or_status(&self) -> String {
        let message = self.message();
        if message.is_empty() {
            format!(
                "native bridge returned status {} without invoking the body",
                self.status
            )
        } else {
            message
        }
    }
}

fn panic_internal_failure(outcome: &NativeOutcome) -> ! {
    let domain = outcome.domain();
    let message = outcome.message_or_status();
    if domain.is_empty() {
        panic!("internal coordination bridge failure: {message}");
    }
    panic!("internal coordination bridge failure ({domain}): {message}");
}

fn bounded_string(buffer: &[c_char]) -> String {
    let bytes = buffer
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect::<Vec<_>>();
    String::from_utf8_lossy(&bytes).into_owned()
}

type AccessBody = unsafe extern "C" fn(*mut c_void);

unsafe extern "C" {
    fn zwirn_coordinated_access(
        path: *const u8,
        path_length: usize,
        intent: i32,
        body: Option<AccessBody>,
        context: *mut c_void,
        outcome: *mut NativeOutcome,
    );
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::os::unix::ffi::OsStringExt;

    use super::*;

    #[test]
    fn real_claim_runs_the_body_once_synchronously_and_preserves_its_result() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("value.txt");
        fs::write(&path, b"before").unwrap();
        let calls = Cell::new(0);
        let mut access = CoordinatedAccess;

        let result = access
            .write(&path, || {
                calls.set(calls.get() + 1);
                fs::write(&path, b"after")?;
                Err::<(), _>(io::Error::other("authoritative body result"))
            })
            .unwrap()
            .unwrap_err();

        assert_eq!(calls.get(), 1);
        assert_eq!(result.to_string(), "authoritative body result");
        assert_eq!(fs::read(path).unwrap(), b"after");
    }

    #[test]
    fn a_missing_target_is_coordinated_and_remains_an_ordinary_body_result() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("missing.lua");
        let mut access = CoordinatedAccess;

        let error = access.read(&path, || fs::read(&path)).unwrap().unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_non_utf8_filesystem_path_crosses_the_native_boundary_without_loss() {
        let workspace = tempfile::tempdir().unwrap();
        let component = OsString::from_vec(b"fragment-\xff.lua".to_vec());
        let path = workspace.path().join(component);
        let expected = path.as_os_str().as_bytes().to_vec();
        let mut access = CoordinatedAccess;

        let bytes = access
            .read(&path, || {
                Ok::<_, std::convert::Infallible>(path.as_os_str().as_bytes().to_vec())
            })
            .unwrap()
            .unwrap();

        assert_eq!(bytes, expected);
    }

    #[test]
    fn a_rust_panic_resumes_only_after_the_native_boundary_returns() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("value.txt");
        fs::write(&path, b"value").unwrap();

        let panic = catch_unwind(AssertUnwindSafe(|| {
            let mut access = CoordinatedAccess;
            let _ = access.read(&path, || -> Result<(), std::convert::Infallible> {
                panic!("deliberate coordinated-body panic");
            });
        }));

        assert!(panic.is_err());
    }

    #[test]
    fn a_changed_accessor_path_is_typed_without_running_or_retrying_the_body() {
        let calls = Cell::new(0);
        let state: BodyState<Result<(), std::convert::Infallible>, _> = BodyState {
            body: Some(|| {
                calls.set(calls.get() + 1);
                Ok::<(), std::convert::Infallible>(())
            }),
            completion: None,
        };
        let mut outcome = NativeOutcome::new();
        outcome.status = ACCESSOR_PATH_CHANGED;

        let error = finish_coordinated(Path::new("named.lua"), AccessKind::Read, state, outcome)
            .unwrap_err();

        assert_eq!(calls.get(), 0);
        assert_eq!(error.kind(), AccessKind::Read);
        assert_eq!(error.path(), Path::new("named.lua"));
        assert_eq!(error.reason(), &CoordinationFailure::AccessorPathChanged);
    }
}
