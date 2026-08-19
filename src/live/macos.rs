//! macOS FSEvents ownership and delivery for a future live-session driver.

use std::ffi::{c_char, c_void};
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::mpsc::{self, Receiver, SyncSender};

use thiserror::Error;

const MONITOR_OK: i32 = 0;
const MONITOR_INVALID_ARGUMENT: i32 = 1;
const MONITOR_ALLOCATION_FAILED: i32 = 2;
const MONITOR_PATH_NOT_REPRESENTABLE: i32 = 3;
const MONITOR_STREAM_CREATE_FAILED: i32 = 4;
const MONITOR_QUEUE_CREATE_FAILED: i32 = 5;
const MONITOR_STREAM_START_FAILED: i32 = 6;

/// One conservative request to resample all configured live-session inputs.
///
/// It deliberately carries no event path, flags, ID, ordering, or batch
/// identity. Every nonempty native delivery attempts to set this pending hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Invalidation;

/// A usable FSEvents stream and its owned callback state.
///
/// The returned receiver already exists while the native stream starts, so an
/// immediate callback can enqueue safely. A future driver owns the separate
/// scheduler-readiness handoff after `start` returns.
pub(crate) struct SessionMonitor {
    native: Option<NonNull<NativeSessionMonitor>>,
    // Native code borrows this stable allocation until `Drop` first stops and
    // drains delivery. Keeping it as a field makes that ownership explicit.
    _callback_state: Box<CallbackState>,
}

impl SessionMonitor {
    /// Starts one current-position stream over the fixed source hierarchy and
    /// the fixed parent of the configured document spelling.
    pub(crate) fn start(
        source_root: &Path,
        document: &Path,
    ) -> Result<(Self, Receiver<Invalidation>), StartError> {
        let (source_root, document_parent) = fixed_scopes(source_root, document)?;
        let native_paths = [
            NativePath::new(&source_root),
            NativePath::new(&document_parent),
        ];
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut callback_state = Box::new(CallbackState { sender });
        let mut outcome = NativeOutcome::new();

        // SAFETY: both path buffers and `outcome` live through this synchronous
        // construction call. The boxed callback state has a stable address and
        // is fully initialized before FSEventStreamStart can begin delivery.
        let native = unsafe {
            zwirn_session_monitor_start(
                native_paths.as_ptr(),
                native_paths.len(),
                Some(deliver_invalidation),
                (&mut *callback_state as *mut CallbackState).cast(),
                &mut outcome,
            )
        };

        let Some(native) = NonNull::new(native) else {
            return Err(StartError::Native(outcome.failure()));
        };
        if outcome.status != MONITOR_OK {
            // A protocol error must still leave no half-live native stream.
            // SAFETY: a non-null return transfers unique ownership to Rust.
            unsafe { zwirn_session_monitor_stop(native.as_ptr()) };
            return Err(StartError::Native(outcome.failure()));
        }

        Ok((
            Self {
                native: Some(native),
                _callback_state: callback_state,
            },
            receiver,
        ))
    }

    #[cfg(test)]
    fn flush_for_evidence(&self) {
        let native = self.native.expect("a live monitor owns its native stream");
        // SAFETY: the native monitor remains live for this shared borrow. This
        // test-only barrier waits until already-occurring events have reached
        // the callback before representative activity begins.
        unsafe { zwirn_session_monitor_flush(native.as_ptr()) };
    }
}

impl Drop for SessionMonitor {
    fn drop(&mut self) {
        if let Some(native) = self.native.take() {
            // SAFETY: this instance uniquely owns the native monitor. Native
            // stop prevents new callbacks and drains its serial delivery queue
            // before returning; only then can `_callback_state` be released.
            unsafe { zwirn_session_monitor_stop(native.as_ptr()) };
        }
    }
}

fn fixed_scopes(source_root: &Path, document: &Path) -> Result<(PathBuf, PathBuf), StartError> {
    let needs_current_directory = !source_root.is_absolute() || !document.is_absolute();
    let current_directory = needs_current_directory
        .then(std::env::current_dir)
        .transpose()
        .map_err(|error| StartError::CurrentDirectory {
            message: error.to_string(),
        })?;
    let source_root = fixed_absolute(source_root, current_directory.as_deref());
    let document = fixed_absolute(document, current_directory.as_deref());
    let document_parent = document
        .parent()
        .ok_or_else(|| StartError::DocumentWithoutParent {
            document: document.clone(),
        })?
        .to_path_buf();
    Ok((source_root, document_parent))
}

fn fixed_absolute(path: &Path, current_directory: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_directory
            .expect("a relative path requires the captured current directory")
            .join(path)
    }
}

struct CallbackState {
    sender: SyncSender<Invalidation>,
}

impl CallbackState {
    fn invalidate(&self) {
        // Empty accepts the dirty state, full already represents it, and
        // disconnected means the receiving driver has begun shutdown.
        let _ = self.sender.try_send(Invalidation);
    }
}

unsafe extern "C" fn deliver_invalidation(context: *mut c_void) {
    // No panic, including one from allocation inside channel delivery, may
    // unwind through the FSEvents C callback.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() {
            return;
        }
        // SAFETY: SessionMonitor owns this allocation until native stop has
        // ended and drained all callback delivery.
        let state = unsafe { &*context.cast::<CallbackState>() };
        state.invalidate();
    }));
}

#[derive(Debug, Error)]
pub(crate) enum StartError {
    #[error("cannot determine the current directory for filesystem monitoring: {message}")]
    CurrentDirectory { message: String },

    #[error("configured document path `{}` has no parent to monitor", document.display())]
    DocumentWithoutParent { document: PathBuf },

    #[error("cannot start macOS filesystem monitoring: {0}")]
    Native(NativeFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeFailure {
    kind: NativeFailureKind,
    message: String,
}

impl fmt::Display for NativeFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            self.kind.fmt(formatter)
        } else {
            write!(formatter, "{}: {}", self.kind, self.message)
        }
    }
}

impl std::error::Error for NativeFailure {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeFailureKind {
    InvalidArgument,
    Allocation,
    PathRepresentation,
    StreamCreation,
    QueueCreation,
    StreamStart,
    Protocol(i32),
}

impl fmt::Display for NativeFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument => formatter.write_str("invalid native argument"),
            Self::Allocation => formatter.write_str("native allocation failed"),
            Self::PathRepresentation => formatter.write_str("native path representation failed"),
            Self::StreamCreation => formatter.write_str("FSEvents stream creation failed"),
            Self::QueueCreation => formatter.write_str("FSEvents queue creation failed"),
            Self::StreamStart => formatter.write_str("FSEvents stream startup failed"),
            Self::Protocol(status) => {
                write!(formatter, "native bridge returned unknown status {status}")
            }
        }
    }
}

#[repr(C)]
struct NativePath {
    bytes: *const u8,
    length: usize,
}

impl NativePath {
    fn new(path: &Path) -> Self {
        let bytes = path.as_os_str().as_bytes();
        Self {
            bytes: bytes.as_ptr(),
            length: bytes.len(),
        }
    }
}

#[repr(C)]
struct NativeOutcome {
    status: i32,
    message: [c_char; 1024],
}

impl NativeOutcome {
    const fn new() -> Self {
        Self {
            status: MONITOR_INVALID_ARGUMENT,
            message: [0; 1024],
        }
    }

    fn failure(&self) -> NativeFailure {
        let kind = match self.status {
            MONITOR_INVALID_ARGUMENT => NativeFailureKind::InvalidArgument,
            MONITOR_ALLOCATION_FAILED => NativeFailureKind::Allocation,
            MONITOR_PATH_NOT_REPRESENTABLE => NativeFailureKind::PathRepresentation,
            MONITOR_STREAM_CREATE_FAILED => NativeFailureKind::StreamCreation,
            MONITOR_QUEUE_CREATE_FAILED => NativeFailureKind::QueueCreation,
            MONITOR_STREAM_START_FAILED => NativeFailureKind::StreamStart,
            status => NativeFailureKind::Protocol(status),
        };
        let bytes = self
            .message
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>();
        NativeFailure {
            kind,
            message: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }
}

#[repr(C)]
struct NativeSessionMonitor {
    _private: [u8; 0],
}

type NativeInvalidation = unsafe extern "C" fn(*mut c_void);

unsafe extern "C" {
    fn zwirn_session_monitor_start(
        paths: *const NativePath,
        path_count: usize,
        invalidated: Option<NativeInvalidation>,
        context: *mut c_void,
        outcome: *mut NativeOutcome,
    ) -> *mut NativeSessionMonitor;

    fn zwirn_session_monitor_stop(monitor: *mut NativeSessionMonitor);

    fn zwirn_session_monitor_flush(monitor: *mut NativeSessionMonitor);
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::ffi::OsStringExt;
    use std::sync::mpsc::TryRecvError;
    use std::time::Duration;

    use super::*;

    const EVENT_DEADLINE: Duration = Duration::from_secs(10);

    #[test]
    fn production_stream_eventually_invalidates_both_required_scopes() {
        let workspace = tempfile::tempdir().unwrap();
        let source_root = workspace.path().join("source-root");
        let nested = source_root.join("nested");
        let documents = workspace.path().join("documents");
        let document = documents.join("patch.audulus4");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(&documents).unwrap();
        fs::write(&document, b"before").unwrap();

        let (monitor, hints) = SessionMonitor::start(&source_root, &document).unwrap();
        monitor.flush_for_evidence();
        drain_preexisting_hints(&hints);
        let fragment = nested.join("voice.lua");
        fs::write(&fragment, b"nested source\n").unwrap();
        hints
            .recv_timeout(EVENT_DEADLINE)
            .expect("nested source activity did not produce an FSEvents invalidation");
        assert_eq!(fs::read(&fragment).unwrap(), b"nested source\n");

        monitor.flush_for_evidence();
        drain_preexisting_hints(&hints);
        let replacement = documents.join("patch.audulus4.tmp");
        fs::write(&replacement, b"after replacement").unwrap();
        fs::rename(&replacement, &document).unwrap();
        hints
            .recv_timeout(EVENT_DEADLINE)
            .expect("document replacement did not produce an FSEvents invalidation");
        assert_eq!(fs::read(&document).unwrap(), b"after replacement");
        drop(monitor);
        assert_sender_released(hints);
    }

    #[test]
    fn an_unrepresentable_scope_is_a_monitor_startup_failure() {
        let workspace = tempfile::tempdir().unwrap();
        let documents = workspace.path().join("documents");
        let document = documents.join("patch.audulus4");
        fs::create_dir(&documents).unwrap();
        fs::write(&document, b"document").unwrap();
        let non_utf8_root = workspace
            .path()
            .join(OsString::from_vec(b"unrepresentable-\xff".to_vec()))
            .join("..")
            .join("source-root");

        match SessionMonitor::start(&non_utf8_root, &document) {
            Err(StartError::Native(failure)) => {
                assert_eq!(failure.kind, NativeFailureKind::PathRepresentation);
            }
            Err(other) => panic!("unexpected startup failure: {other}"),
            Ok((monitor, _)) => {
                drop(monitor);
                panic!("an unrepresentable scope must not report successful startup");
            }
        }
    }

    #[test]
    fn pending_invalidations_collapse_and_the_latch_refills_after_consumption() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut state = CallbackState { sender };

        deliver_for_test(&mut state);
        deliver_for_test(&mut state);
        assert_eq!(receiver.try_recv(), Ok(Invalidation));
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));

        deliver_for_test(&mut state);
        assert_eq!(receiver.try_recv(), Ok(Invalidation));
    }

    fn deliver_for_test(state: &mut CallbackState) {
        // SAFETY: this synchronous call receives the exact live callback state
        // and returns before the stack allocation can be released.
        unsafe { deliver_invalidation((state as *mut CallbackState).cast()) };
    }

    fn assert_sender_released(receiver: Receiver<Invalidation>) {
        loop {
            match receiver.try_recv() {
                Ok(Invalidation) => {}
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {
                    panic!("monitor drop returned before releasing callback state")
                }
            }
        }
    }

    fn drain_preexisting_hints(receiver: &Receiver<Invalidation>) {
        loop {
            match receiver.try_recv() {
                Ok(Invalidation) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    panic!("a live monitor released its callback state")
                }
            }
        }
    }
}
