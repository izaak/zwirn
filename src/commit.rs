//! Ordered, direct filesystem writes for fully prepared synchronization outputs.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[cfg(test)]
use crate::access::DirectAccess;
use crate::access::{AccessPolicy, PolicyFailure, flatten};
use crate::fragment::FragmentPath;
use crate::source_root::{SourceRoot, relative_path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExternalWrite {
    CreateNew,
    CreateOrTruncate,
}

/// A fully prepared external fragment output.
pub(crate) struct ExternalOutput<'a> {
    pub path: &'a FragmentPath,
    pub bytes: &'a [u8],
    pub write: ExternalWrite,
}

/// A fully prepared document output.
pub(crate) struct DocumentOutput<'a> {
    pub destination: &'a Path,
    pub bytes: &'a [u8],
}

#[derive(Debug)]
pub(crate) struct CommitAccessFailure<A> {
    pub(crate) source: A,
    pub(crate) completed: Vec<FragmentPath>,
}

/// `external` must be strictly ordered by canonical fragment path. External
/// files are written in that order, followed by the document.
#[cfg(test)]
pub(crate) fn commit(
    source_root: &SourceRoot,
    external: &[ExternalOutput<'_>],
    absent_targets: &[&FragmentPath],
    document: Option<DocumentOutput<'_>>,
) -> Result<(), CommitError> {
    let mut access = DirectAccess;
    direct_commit_result(commit_with_access(
        &mut access,
        source_root,
        external,
        absent_targets,
        document,
    ))
}

pub(crate) fn commit_with_access<P: AccessPolicy>(
    access: &mut P,
    source_root: &SourceRoot,
    external: &[ExternalOutput<'_>],
    absent_targets: &[&FragmentPath],
    document: Option<DocumentOutput<'_>>,
) -> Result<(), PolicyFailure<CommitAccessFailure<P::Error>, CommitError>> {
    commit_using(
        access,
        &mut RealFilesystem { source_root },
        external,
        absent_targets,
        document,
    )
}

#[cfg(test)]
fn commit_with<F: CommitFilesystem>(
    filesystem: &mut F,
    external: &[ExternalOutput<'_>],
    absent_targets: &[&FragmentPath],
    document: Option<DocumentOutput<'_>>,
) -> Result<(), CommitError> {
    let mut access = DirectAccess;
    direct_commit_result(commit_using(
        &mut access,
        filesystem,
        external,
        absent_targets,
        document,
    ))
}

#[cfg(test)]
fn direct_commit_result(
    result: Result<(), PolicyFailure<CommitAccessFailure<std::convert::Infallible>, CommitError>>,
) -> Result<(), CommitError> {
    match result {
        Ok(()) => Ok(()),
        Err(PolicyFailure::Operation(error)) => Err(error),
        Err(PolicyFailure::Access(CommitAccessFailure { source, .. })) => match source {},
    }
}

fn commit_using<P: AccessPolicy, F: CommitFilesystem>(
    access: &mut P,
    filesystem: &mut F,
    external: &[ExternalOutput<'_>],
    absent_targets: &[&FragmentPath],
    document: Option<DocumentOutput<'_>>,
) -> Result<(), PolicyFailure<CommitAccessFailure<P::Error>, CommitError>> {
    validate_order(external).map_err(PolicyFailure::Operation)?;

    let mut completed = Vec::new();
    completed
        .try_reserve_exact(external.len())
        .map_err(|_| PolicyFailure::Operation(CommitFailure::AllocationFailed.before_writes()))?;
    for output in external {
        if let Some(parent) = nonempty_parent(relative_path(output.path))
            && let Err(source) = filesystem.create_dir_all(parent)
        {
            return Err(PolicyFailure::Operation(
                CommitFailure::CreateExternalParent {
                    path: output.path.clone(),
                    destination: filesystem.external_destination(output.path),
                    source,
                }
                .after(completed),
            ));
        }
        let destination = filesystem.external_destination(output.path);
        let aliased_target = match flatten(access.write(&destination, || {
            filesystem.write_external(output.path, output.bytes, output.write, absent_targets)
        })) {
            Ok(aliased_target) => aliased_target,
            Err(PolicyFailure::Access(source)) => {
                return Err(PolicyFailure::Access(CommitAccessFailure {
                    source,
                    completed,
                }));
            }
            Err(PolicyFailure::Operation(source)) => {
                return Err(PolicyFailure::Operation(
                    CommitFailure::WriteExternal {
                        path: output.path.clone(),
                        destination,
                        source,
                    }
                    .after(completed),
                ));
            }
        };
        completed.push(output.path.clone());

        if let Some(other_path) = aliased_target {
            return Err(PolicyFailure::Operation(
                CommitFailure::CreatedTargetAlias {
                    created_path: output.path.clone(),
                    other_path,
                }
                .after(completed),
            ));
        }
    }

    if let Some(document) = document {
        match flatten(access.write(document.destination, || {
            filesystem.write_document(document.destination, document.bytes)
        })) {
            Ok(()) => {}
            Err(PolicyFailure::Access(source)) => {
                return Err(PolicyFailure::Access(CommitAccessFailure {
                    source,
                    completed,
                }));
            }
            Err(PolicyFailure::Operation(source)) => {
                return Err(PolicyFailure::Operation(
                    CommitFailure::WriteDocument {
                        destination: document.destination.to_owned(),
                        source,
                    }
                    .after(completed),
                ));
            }
        }
    }
    Ok(())
}

fn validate_order(external: &[ExternalOutput<'_>]) -> Result<(), CommitError> {
    for pair in external.windows(2) {
        if pair[0].path >= pair[1].path {
            return Err(CommitFailure::ExternalOrder {
                previous: pair[0].path.clone(),
                next: pair[1].path.clone(),
            }
            .before_writes());
        }
    }
    Ok(())
}

fn nonempty_parent(path: &Path) -> Option<&Path> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

trait CommitFilesystem {
    fn external_destination(&self, path: &FragmentPath) -> PathBuf;
    fn create_dir_all(&mut self, path: &Path) -> io::Result<()>;
    fn write_external(
        &mut self,
        path: &FragmentPath,
        bytes: &[u8],
        write: ExternalWrite,
        absent_targets: &[&FragmentPath],
    ) -> io::Result<Option<FragmentPath>>;
    fn write_document(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()>;
}

struct RealFilesystem<'a> {
    source_root: &'a SourceRoot,
}

impl CommitFilesystem for RealFilesystem<'_> {
    fn external_destination(&self, path: &FragmentPath) -> PathBuf {
        self.source_root.named_path(relative_path(path))
    }

    fn create_dir_all(&mut self, path: &Path) -> io::Result<()> {
        self.source_root.create_dir_all(path)
    }

    fn write_external(
        &mut self,
        path: &FragmentPath,
        bytes: &[u8],
        write: ExternalWrite,
        absent_targets: &[&FragmentPath],
    ) -> io::Result<Option<FragmentPath>> {
        match write {
            ExternalWrite::CreateNew => self.source_root.create_target(path, bytes, absent_targets),
            ExternalWrite::CreateOrTruncate => {
                self.source_root.replace_target(path, bytes)?;
                Ok(None)
            }
        }
    }

    fn write_document(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        fs::write(path, bytes)
    }
}

/// A commit failure together with external writes that completed before it.
#[derive(Debug)]
pub struct CommitError {
    completed: Vec<FragmentPath>,
    failure: CommitFailure,
}

impl CommitError {
    /// External fragments whose complete prepared bytes were written.
    pub fn completed(&self) -> &[FragmentPath] {
        &self.completed
    }

    /// The operation that failed.
    pub fn failure(&self) -> &CommitFailure {
        &self.failure
    }
}

impl fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.failure.fmt(formatter)?;
        if !self.completed.is_empty() {
            formatter.write_str(" after writing external fragment")?;
            if self.completed.len() != 1 {
                formatter.write_str("s")?;
            }
            formatter.write_str(" ")?;
            for (index, path) in self.completed.iter().enumerate() {
                if index != 0 {
                    formatter.write_str(", ")?;
                }
                write!(formatter, "`{path}`")?;
            }
        }
        Ok(())
    }
}

impl Error for CommitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.failure)
    }
}

/// The stage and destination at which a commit failed.
#[derive(Debug, Error)]
pub enum CommitFailure {
    #[error("memory could not be reserved for commit reporting")]
    AllocationFailed,

    #[error("external outputs are not strictly ordered: `{previous}` precedes `{next}`")]
    ExternalOrder {
        previous: FragmentPath,
        next: FragmentPath,
    },

    #[error(
        "cannot create the parent of external fragment `{path}` at `{}`: {source}",
        destination.display()
    )]
    CreateExternalParent {
        path: FragmentPath,
        destination: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("cannot write external fragment `{path}` at `{}`: {source}", destination.display())]
    WriteExternal {
        path: FragmentPath,
        destination: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error(
        "created external fragment `{created_path}` also identifies fragment target `{other_path}`"
    )]
    CreatedTargetAlias {
        created_path: FragmentPath,
        other_path: FragmentPath,
    },

    #[error("cannot write document `{}`: {source}", destination.display())]
    WriteDocument {
        destination: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl CommitFailure {
    fn before_writes(self) -> CommitError {
        self.after(Vec::new())
    }

    fn after(self, completed: Vec<FragmentPath>) -> CommitError {
        CommitError {
            completed,
            failure: self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_existing_and_missing_external_files_then_the_document() {
        let workspace = tempfile::tempdir().unwrap();
        let existing = workspace.path().join("a.lua");
        let missing = workspace.path().join("nested/b.lua");
        let document = workspace.path().join("patch.audulus4");
        fs::write(&existing, b"old external content").unwrap();
        fs::write(&document, b"old document").unwrap();
        let source_root = SourceRoot::open(workspace.path()).unwrap();

        let a = FragmentPath::try_from("a.lua").unwrap();
        let b = FragmentPath::try_from("nested/b.lua").unwrap();
        let external = [
            ExternalOutput {
                path: &a,
                bytes: b"new a\n",
                write: ExternalWrite::CreateOrTruncate,
            },
            ExternalOutput {
                path: &b,
                bytes: b"new b\n",
                write: ExternalWrite::CreateNew,
            },
        ];

        commit(
            &source_root,
            &external,
            &[&b],
            Some(DocumentOutput {
                destination: &document,
                bytes: b"new document",
            }),
        )
        .unwrap();

        assert_eq!(fs::read(&existing).unwrap(), b"new a\n");
        assert_eq!(fs::read(&missing).unwrap(), b"new b\n");
        assert_eq!(fs::read(&document).unwrap(), b"new document");
    }

    #[test]
    fn exclusive_creation_refuses_an_occupied_destination_before_the_document() {
        let workspace = tempfile::tempdir().unwrap();
        let occupied = workspace.path().join("fragment.lua");
        let document = workspace.path().join("patch.audulus4");
        fs::write(&occupied, b"appeared after discovery").unwrap();
        fs::write(&document, b"old document").unwrap();
        let source_root = SourceRoot::open(workspace.path()).unwrap();

        let path = FragmentPath::try_from("fragment.lua").unwrap();
        let external = [ExternalOutput {
            path: &path,
            bytes: b"prepared fragment\n",
            write: ExternalWrite::CreateNew,
        }];

        let error = commit(
            &source_root,
            &external,
            &[&path],
            Some(DocumentOutput {
                destination: &document,
                bytes: b"new document",
            }),
        )
        .unwrap_err();

        assert!(error.completed().is_empty());
        assert!(matches!(
            error.failure(),
            CommitFailure::WriteExternal {
                path: failed,
                destination,
                ..
            } if failed == &path && destination == &occupied
        ));
        assert_eq!(fs::read(occupied).unwrap(), b"appeared after discovery");
        assert_eq!(fs::read(document).unwrap(), b"old document");
    }

    #[cfg(unix)]
    #[test]
    fn an_escaping_parent_at_commit_cannot_reach_an_outside_file_or_the_document() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let root_path = workspace.path().join("sources");
        let outside = workspace.path().join("outside");
        fs::create_dir(&root_path).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("target.lua"), b"outside").unwrap();
        let document = workspace.path().join("patch.audulus4");
        fs::write(&document, b"old document").unwrap();

        let source_root = SourceRoot::open(&root_path).unwrap();
        symlink("../outside", root_path.join("z-escape")).unwrap();

        let safe = FragmentPath::try_from("a-safe.lua").unwrap();
        let escape = FragmentPath::try_from("z-escape/target.lua").unwrap();
        let safe_destination = root_path.join(safe.as_str());
        let external = [
            ExternalOutput {
                path: &safe,
                bytes: b"safe\n",
                write: ExternalWrite::CreateNew,
            },
            ExternalOutput {
                path: &escape,
                bytes: b"escaped\n",
                write: ExternalWrite::CreateOrTruncate,
            },
        ];

        let error = commit(
            &source_root,
            &external,
            &[&safe],
            Some(DocumentOutput {
                destination: &document,
                bytes: b"new document",
            }),
        )
        .unwrap_err();

        assert_eq!(error.completed(), std::slice::from_ref(&safe));
        assert!(matches!(
            error.failure(),
            CommitFailure::CreateExternalParent { path, .. }
                | CommitFailure::WriteExternal { path, .. }
                if path == &escape
        ));
        assert_eq!(fs::read(safe_destination).unwrap(), b"safe\n");
        assert_eq!(fs::read(outside.join("target.lua")).unwrap(), b"outside");
        assert_eq!(fs::read(document).unwrap(), b"old document");
    }

    #[test]
    fn stops_in_canonical_order_and_reports_only_completed_external_writes() {
        let b_destination = Path::new("out/b");
        let a = FragmentPath::try_from("out/a").unwrap();
        let b = FragmentPath::try_from("out/b").unwrap();
        let c = FragmentPath::try_from("out/c").unwrap();
        let external = [
            ExternalOutput {
                path: &a,
                bytes: b"a",
                write: ExternalWrite::CreateOrTruncate,
            },
            ExternalOutput {
                path: &b,
                bytes: b"b",
                write: ExternalWrite::CreateOrTruncate,
            },
            ExternalOutput {
                path: &c,
                bytes: b"c",
                write: ExternalWrite::CreateOrTruncate,
            },
        ];
        let mut filesystem = RecordingFilesystem {
            events: Vec::new(),
            fail_write: Some(b_destination.to_owned()),
        };

        let error = commit_with(&mut filesystem, &external, &[], None).unwrap_err();

        assert_eq!(error.completed(), std::slice::from_ref(&a));
        assert!(matches!(
            error.failure(),
            CommitFailure::WriteExternal { path, .. } if path == &b
        ));
        assert_eq!(
            filesystem.events,
            ["mkdir:out", "write:out/a", "mkdir:out", "write:out/b",]
        );
        assert!(
            error
                .to_string()
                .contains("after writing external fragment `out/a`")
        );
    }

    #[test]
    fn rejects_noncanonical_output_order_before_filesystem_effects() {
        let a = FragmentPath::try_from("a").unwrap();
        let b = FragmentPath::try_from("b").unwrap();
        let external = [
            ExternalOutput {
                path: &b,
                bytes: b"b",
                write: ExternalWrite::CreateOrTruncate,
            },
            ExternalOutput {
                path: &a,
                bytes: b"a",
                write: ExternalWrite::CreateOrTruncate,
            },
        ];
        let mut filesystem = RecordingFilesystem {
            events: Vec::new(),
            fail_write: None,
        };

        let error = commit_with(&mut filesystem, &external, &[], None).unwrap_err();

        assert!(matches!(
            error.failure(),
            CommitFailure::ExternalOrder { previous, next }
                if previous == &b && next == &a
        ));
        assert!(filesystem.events.is_empty());
    }

    #[test]
    fn a_document_write_failure_leaves_all_completed_external_outputs_reported() {
        let document = PathBuf::from("patch.audulus4");
        let a = FragmentPath::try_from("a").unwrap();
        let b = FragmentPath::try_from("b").unwrap();
        let external = [
            ExternalOutput {
                path: &a,
                bytes: b"a",
                write: ExternalWrite::CreateOrTruncate,
            },
            ExternalOutput {
                path: &b,
                bytes: b"b",
                write: ExternalWrite::CreateOrTruncate,
            },
        ];
        let mut filesystem = RecordingFilesystem {
            events: Vec::new(),
            fail_write: Some(document.clone()),
        };

        let error = commit_with(
            &mut filesystem,
            &external,
            &[],
            Some(DocumentOutput {
                destination: &document,
                bytes: b"new document",
            }),
        )
        .unwrap_err();

        assert_eq!(error.completed(), &[a, b]);
        assert!(matches!(
            error.failure(),
            CommitFailure::WriteDocument { destination, .. } if destination == &document
        ));
        assert_eq!(
            filesystem.events.last(),
            Some(&format!("write:{}", document.display()))
        );
    }

    #[test]
    fn document_policy_refusal_retains_completed_external_outputs_without_running_its_body() {
        let document = PathBuf::from("patch.audulus4");
        let fragment = FragmentPath::try_from("fragment.lua").unwrap();
        let external = [ExternalOutput {
            path: &fragment,
            bytes: b"fragment",
            write: ExternalWrite::CreateOrTruncate,
        }];
        let mut filesystem = RecordingFilesystem {
            events: Vec::new(),
            fail_write: None,
        };
        let mut access = RefuseAccessTo {
            path: document.clone(),
        };

        let error = commit_using(
            &mut access,
            &mut filesystem,
            &external,
            &[],
            Some(DocumentOutput {
                destination: &document,
                bytes: b"document",
            }),
        )
        .unwrap_err();

        let PolicyFailure::Access(CommitAccessFailure { source, completed }) = error else {
            panic!("expected policy-access refusal");
        };
        assert_eq!(source, AccessRefused);
        assert_eq!(completed, vec![fragment]);
        assert_eq!(
            filesystem.events,
            ["write:fragment.lua"],
            "the refused document-write body must not run"
        );
    }

    #[derive(Debug, Eq, PartialEq)]
    struct AccessRefused;

    struct RefuseAccessTo {
        path: PathBuf,
    }

    impl AccessPolicy for RefuseAccessTo {
        type Error = AccessRefused;

        fn read<T, E>(
            &mut self,
            _named_path: &Path,
            body: impl FnOnce() -> Result<T, E>,
        ) -> Result<Result<T, E>, Self::Error> {
            Ok(body())
        }

        fn write<T, E>(
            &mut self,
            named_path: &Path,
            body: impl FnOnce() -> Result<T, E>,
        ) -> Result<Result<T, E>, Self::Error> {
            if named_path == self.path {
                Err(AccessRefused)
            } else {
                Ok(body())
            }
        }
    }

    struct RecordingFilesystem {
        events: Vec<String>,
        fail_write: Option<PathBuf>,
    }

    impl RecordingFilesystem {
        fn record(&mut self, operation: &str, path: &Path) {
            self.events.push(format!("{operation}:{}", path.display()));
        }

        fn write_external(&mut self, path: &FragmentPath) -> io::Result<()> {
            let path = relative_path(path);
            self.record("write", path);
            if self.fail_write.as_deref() == Some(path) {
                return Err(io::Error::other("injected failure"));
            }
            Ok(())
        }
    }

    impl CommitFilesystem for RecordingFilesystem {
        fn external_destination(&self, path: &FragmentPath) -> PathBuf {
            relative_path(path).to_owned()
        }

        fn create_dir_all(&mut self, path: &Path) -> io::Result<()> {
            self.record("mkdir", path);
            Ok(())
        }

        fn write_external(
            &mut self,
            path: &FragmentPath,
            _bytes: &[u8],
            _write: ExternalWrite,
            _absent_targets: &[&FragmentPath],
        ) -> io::Result<Option<FragmentPath>> {
            self.write_external(path)?;
            Ok(None)
        }

        fn write_document(&mut self, path: &Path, _bytes: &[u8]) -> io::Result<()> {
            self.record("write", path);
            if self.fail_write.as_deref() == Some(path) {
                return Err(io::Error::other("injected failure"));
            }
            Ok(())
        }
    }
}
