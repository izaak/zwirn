//! Foreground macOS live-session assembly and deterministic scheduling.
//!
//! The driver owns one bounded wake channel, FSEvents monitoring, signal
//! iteration, diagnostics, and the synchronous calls into the ordinary sync
//! engine. The foreground loop orders those effects only at synchronous run
//! boundaries.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};

use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::iterator::{Handle as SignalHandle, Signals};
use thiserror::Error as ThisError;
use zwirn::engine::{Error as EngineError, Mode, Report, ReportEntry, Request, execute};
use zwirn::reconcile::Operation;

mod macos;

use self::macos::SessionMonitor;

/// One bounded prompt for the foreground driver to inspect boundary state.
///
/// Filesystem wakes carry the invalidation latch themselves. Control wakes are
/// accompanied by the separate shutdown latch, so a full channel cannot lose
/// a signal request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Wake {
    Filesystem,
    Control,
}

/// A failure that prevents the foreground live session from continuing.
#[derive(Debug, ThisError)]
pub(crate) enum Error {
    #[error("document path `{}` does not have the `.audulus4` extension", path.display())]
    InvalidDocumentExtension { path: PathBuf },

    #[error(transparent)]
    Monitor(#[from] macos::StartError),

    #[error("cannot register SIGINT and SIGTERM handlers: {source}")]
    SignalRegistration {
        #[source]
        source: io::Error,
    },

    #[error("cannot start the shutdown-signal thread: {source}")]
    SignalThreadStart {
        #[source]
        source: io::Error,
    },

    #[error("the shutdown-signal thread stopped unexpectedly")]
    SignalThreadStopped,

    #[error("live-session event delivery stopped unexpectedly")]
    EventDeliveryStopped,

    #[error("cannot write live-session diagnostics: {source}")]
    Diagnostic {
        #[source]
        source: io::Error,
    },
}

/// Runs one foreground live session until an orderly signal request.
pub(crate) fn run(cwd: &Path, document: &Path, source_root: Option<&Path>) -> Result<(), Error> {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    run_with_writer(cwd, document, source_root, &mut stderr)
}

fn run_with_writer(
    cwd: &Path,
    document: &Path,
    source_root: Option<&Path>,
    diagnostics: &mut impl Write,
) -> Result<(), Error> {
    validate_document_path(document)?;

    let (wake_sender, wake_receiver) = mpsc::sync_channel(1);
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut signals = ShutdownSignals::start(Arc::clone(&shutdown), wake_sender.clone())?;

    let monitored_document = fixed_path(cwd, document);
    let monitored_source_root = source_root
        .map(|path| fixed_path(cwd, path))
        .unwrap_or_else(|| monitored_document.parent().unwrap_or(cwd).to_path_buf());
    let monitor = match SessionMonitor::start(
        &monitored_source_root,
        &monitored_document,
        wake_sender.clone(),
    ) {
        Ok(monitor) => monitor,
        Err(error) => {
            let _ = signals.close_and_join();
            return Err(error.into());
        }
    };
    drop(wake_sender);

    let request = Request {
        cwd,
        document,
        source_root,
        selectors: &[],
        mode: Mode::Mutate(Operation::Sync),
    };
    let mut reporter = Diagnostics::new(diagnostics);
    let session_result = reporter
        .started(document)
        .and_then(|()| Driver::new(request, wake_receiver, shutdown).run(&mut reporter));

    // Native callbacks stop before the signal iterator is closed and joined.
    // Dropping its final handle then unregisters the installed actions; the
    // dispositions they replaced remain effectively ignored during imminent
    // process exit.
    drop(monitor);
    let signal_result = signals.close_and_join();
    match session_result {
        Err(error) => Err(error),
        Ok(()) => signal_result,
    }
}

fn validate_document_path(document: &Path) -> Result<(), Error> {
    if document.extension() == Some(OsStr::new("audulus4")) {
        Ok(())
    } else {
        Err(Error::InvalidDocumentExtension {
            path: document.to_path_buf(),
        })
    }
}

fn fixed_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

struct Driver<'a> {
    request: Request<'a>,
    wakes: Receiver<Wake>,
    shutdown: Arc<AtomicBool>,
}

impl<'a> Driver<'a> {
    fn new(request: Request<'a>, wakes: Receiver<Wake>, shutdown: Arc<AtomicBool>) -> Self {
        Self {
            request,
            wakes,
            shutdown,
        }
    }

    fn run(&mut self, diagnostics: &mut Diagnostics<'_, impl Write>) -> Result<(), Error> {
        // Signal handling and monitoring are both live before this immediate
        // initial decision. Observing shutdown here orders it before the run.
        let mut next = if self.shutdown_requested() {
            Next::Stop
        } else {
            Next::Reconcile
        };

        loop {
            match next {
                Next::Reconcile => {
                    // Choosing this branch is the run's ordering point. A
                    // signal received afterward cannot cancel this call.
                    diagnostics.reconciliation(execute(self.request))?;
                    next = self.next_after_reconciliation()?;
                }
                Next::Wait => next = self.wait_for_boundary()?,
                Next::Stop => return diagnostics.stopped(),
            }
        }
    }

    fn next_after_reconciliation(&self) -> Result<Next, Error> {
        if self.shutdown_requested() {
            return Ok(Next::Stop);
        }
        let wake = match self.wakes.try_recv() {
            Ok(wake) => Some(wake),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => return Err(Error::EventDeliveryStopped),
        };
        Ok(boundary_next(self.shutdown_requested(), wake))
    }

    fn wait_for_boundary(&self) -> Result<Next, Error> {
        loop {
            if self.shutdown_requested() {
                return Ok(Next::Stop);
            }
            let wake = self.wakes.recv().map_err(|_| Error::EventDeliveryStopped)?;
            let next = boundary_next(self.shutdown_requested(), Some(wake));
            if next != Next::Wait {
                return Ok(next);
            }
        }
    }

    fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Next {
    Reconcile,
    Wait,
    Stop,
}

fn boundary_next(shutdown: bool, wake: Option<Wake>) -> Next {
    if shutdown || wake == Some(Wake::Control) {
        Next::Stop
    } else if wake == Some(Wake::Filesystem) {
        Next::Reconcile
    } else {
        Next::Wait
    }
}

struct ShutdownSignals {
    handle: Option<SignalHandle>,
    thread: Option<JoinHandle<()>>,
}

impl ShutdownSignals {
    fn start(shutdown: Arc<AtomicBool>, wake: SyncSender<Wake>) -> Result<Self, Error> {
        let mut signals = Signals::new([SIGINT, SIGTERM])
            .map_err(|source| Error::SignalRegistration { source })?;
        let handle = signals.handle();
        let thread = thread::Builder::new()
            .name("zwirn-live-signals".to_owned())
            .spawn(move || {
                for signal in signals.forever() {
                    debug_assert!(signal == SIGINT || signal == SIGTERM);
                    if !shutdown.swap(true, Ordering::SeqCst) {
                        let _ = wake.try_send(Wake::Control);
                    }
                }
            })
            .map_err(|source| Error::SignalThreadStart { source })?;
        Ok(Self {
            handle: Some(handle),
            thread: Some(thread),
        })
    }

    fn close_and_join(&mut self) -> Result<(), Error> {
        if let Some(handle) = self.handle.as_ref() {
            handle.close();
        }
        let joined = self.thread.take().map(JoinHandle::join);
        drop(self.handle.take());

        match joined {
            Some(Ok(())) | None => Ok(()),
            Some(Err(_)) => Err(Error::SignalThreadStopped),
        }
    }
}

impl Drop for ShutdownSignals {
    fn drop(&mut self) {
        let _ = self.close_and_join();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Blocker {
    States(Vec<String>),
    Failure(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlockerTransition {
    Unchanged,
    Report,
    Recovered,
    RecoveredAndReport,
}

fn blocker_transition(previous: Option<&Blocker>, current: Option<&Blocker>) -> BlockerTransition {
    match (previous, current) {
        (None, None) | (Some(_), Some(_)) if previous == current => BlockerTransition::Unchanged,
        (Some(_), None) => BlockerTransition::Recovered,
        (Some(Blocker::Failure(_)), Some(Blocker::States(_))) => {
            BlockerTransition::RecoveredAndReport
        }
        _ => BlockerTransition::Report,
    }
}

struct Diagnostics<'a, W> {
    writer: &'a mut W,
    blocker: Option<Blocker>,
}

impl<'a, W: Write> Diagnostics<'a, W> {
    fn new(writer: &'a mut W) -> Self {
        Self {
            writer,
            blocker: None,
        }
    }

    fn started(&mut self, document: &Path) -> Result<(), Error> {
        writeln!(
            self.writer,
            "zwirn live: monitoring `{}`",
            document.display()
        )
        .map_err(|source| Error::Diagnostic { source })
    }

    fn stopped(&mut self) -> Result<(), Error> {
        writeln!(self.writer, "zwirn live: stopped").map_err(|source| Error::Diagnostic { source })
    }

    fn reconciliation(&mut self, result: Result<Report, EngineError>) -> Result<(), Error> {
        let current = match result {
            Ok(report) => {
                let mut states = Vec::new();
                for entry in &report.entries {
                    match entry {
                        ReportEntry::Action { .. } => {
                            writeln!(
                                self.writer,
                                "zwirn live: {}\t{}",
                                entry.path(),
                                entry.result()
                            )
                            .map_err(|source| Error::Diagnostic { source })?;
                        }
                        ReportEntry::State { .. } => {
                            states.push(format!("{}\t{}", entry.path(), entry.result()));
                        }
                    }
                }
                (!states.is_empty()).then_some(Blocker::States(states))
            }
            Err(error) => {
                for path in error.committed_external() {
                    writeln!(
                        self.writer,
                        "zwirn live: external file already written for `{path}`"
                    )
                    .map_err(|source| Error::Diagnostic { source })?;
                }
                Some(Blocker::Failure(engine_failure(&error)))
            }
        };

        match blocker_transition(self.blocker.as_ref(), current.as_ref()) {
            BlockerTransition::Unchanged => {}
            BlockerTransition::Recovered => {
                writeln!(self.writer, "zwirn live: reconciliation recovered")
                    .map_err(|source| Error::Diagnostic { source })?;
            }
            BlockerTransition::RecoveredAndReport => {
                writeln!(self.writer, "zwirn live: reconciliation recovered")
                    .map_err(|source| Error::Diagnostic { source })?;
                self.report_blocker(current.as_ref())?;
            }
            BlockerTransition::Report => self.report_blocker(current.as_ref())?,
        }
        self.blocker = current;
        Ok(())
    }

    fn report_blocker(&mut self, blocker: Option<&Blocker>) -> Result<(), Error> {
        match blocker {
            Some(Blocker::States(states)) => {
                for state in states {
                    writeln!(self.writer, "zwirn live: attention: {state}")
                        .map_err(|source| Error::Diagnostic { source })?;
                }
            }
            Some(Blocker::Failure(failure)) => {
                writeln!(self.writer, "zwirn live: blocked: {failure}")
                    .map_err(|source| Error::Diagnostic { source })?;
            }
            None => {}
        }
        Ok(())
    }
}

fn engine_failure(error: &EngineError) -> String {
    match error {
        EngineError::Commit(error) => error.failure().to_string(),
        EngineError::Coordination(error) => error.failure().to_string(),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_report_changed_blockers_and_full_recovery_only() {
        let first = Blocker::Failure("first".to_owned());
        let second = Blocker::Failure("second".to_owned());

        assert_eq!(
            blocker_transition(None, Some(&first)),
            BlockerTransition::Report
        );
        assert_eq!(
            blocker_transition(Some(&first), Some(&first)),
            BlockerTransition::Unchanged
        );
        assert_eq!(
            blocker_transition(Some(&first), Some(&second)),
            BlockerTransition::Report
        );
        let attention = Blocker::States(vec!["path\tmissing".to_owned()]);
        assert_eq!(
            blocker_transition(Some(&second), Some(&attention)),
            BlockerTransition::RecoveredAndReport
        );
        assert_eq!(
            blocker_transition(Some(&attention), None),
            BlockerTransition::Recovered
        );
        assert_eq!(blocker_transition(None, None), BlockerTransition::Unchanged);
    }

    #[test]
    fn synchronous_boundaries_prioritize_shutdown_and_require_a_dirty_hint_to_run() {
        assert_eq!(boundary_next(false, None), Next::Wait);
        assert_eq!(
            boundary_next(false, Some(Wake::Filesystem)),
            Next::Reconcile
        );
        assert_eq!(boundary_next(false, Some(Wake::Control)), Next::Stop);
        assert_eq!(boundary_next(true, None), Next::Stop);
        assert_eq!(boundary_next(true, Some(Wake::Filesystem)), Next::Stop);
    }
}
