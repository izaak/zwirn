//! Ordered, direct filesystem writes for fully prepared synchronization outputs.

use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::fragment::FragmentPath;

/// How a prepared external output opens its destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalWrite {
    CreateNew,
    CreateOrTruncate,
}

/// A fully prepared external fragment output.
pub struct ExternalOutput<'a> {
    pub path: &'a FragmentPath,
    pub destination: &'a Path,
    pub bytes: &'a [u8],
    pub write: ExternalWrite,
}

/// A fully prepared document output.
pub struct DocumentOutput<'a> {
    pub destination: &'a Path,
    pub bytes: &'a [u8],
}

/// `external` must be strictly ordered by canonical fragment path. External
/// files are written in that order, followed by the document.
pub fn commit(
    external: &[ExternalOutput<'_>],
    document: Option<DocumentOutput<'_>>,
) -> Result<(), CommitError> {
    commit_with(&mut RealFilesystem, external, document)
}

fn commit_with<F: CommitFilesystem>(
    filesystem: &mut F,
    external: &[ExternalOutput<'_>],
    document: Option<DocumentOutput<'_>>,
) -> Result<(), CommitError> {
    validate_order(external)?;

    let mut completed = Vec::new();
    completed
        .try_reserve_exact(external.len())
        .map_err(|_| CommitFailure::AllocationFailed.before_writes())?;
    for output in external {
        if let Some(parent) = nonempty_parent(output.destination)
            && let Err(source) = filesystem.create_dir_all(parent)
        {
            return Err(CommitFailure::CreateExternalParent {
                path: output.path.clone(),
                destination: output.destination.to_owned(),
                source,
            }
            .after(completed));
        }
        if let Err(source) = filesystem.write(output.destination, output.bytes, output.write) {
            return Err(CommitFailure::WriteExternal {
                path: output.path.clone(),
                destination: output.destination.to_owned(),
                source,
            }
            .after(completed));
        }
        completed.push(output.path.clone());
    }

    if let Some(document) = document
        && let Err(source) = filesystem.write(
            document.destination,
            document.bytes,
            ExternalWrite::CreateOrTruncate,
        )
    {
        return Err(CommitFailure::WriteDocument {
            destination: document.destination.to_owned(),
            source,
        }
        .after(completed));
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
    fn create_dir_all(&mut self, path: &Path) -> io::Result<()>;
    fn write(&mut self, path: &Path, bytes: &[u8], write: ExternalWrite) -> io::Result<()>;
}

struct RealFilesystem;

impl CommitFilesystem for RealFilesystem {
    fn create_dir_all(&mut self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    fn write(&mut self, path: &Path, bytes: &[u8], write: ExternalWrite) -> io::Result<()> {
        match write {
            ExternalWrite::CreateNew => OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?
                .write_all(bytes),
            ExternalWrite::CreateOrTruncate => fs::write(path, bytes),
        }
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

        let a = FragmentPath::try_from("a.lua").unwrap();
        let b = FragmentPath::try_from("nested/b.lua").unwrap();
        let external = [
            ExternalOutput {
                path: &a,
                destination: &existing,
                bytes: b"new a\n",
                write: ExternalWrite::CreateOrTruncate,
            },
            ExternalOutput {
                path: &b,
                destination: &missing,
                bytes: b"new b\n",
                write: ExternalWrite::CreateNew,
            },
        ];

        commit(
            &external,
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

        let path = FragmentPath::try_from("fragment.lua").unwrap();
        let external = [ExternalOutput {
            path: &path,
            destination: &occupied,
            bytes: b"prepared fragment\n",
            write: ExternalWrite::CreateNew,
        }];

        let error = commit(
            &external,
            Some(DocumentOutput {
                destination: &document,
                bytes: b"new document",
            }),
        )
        .unwrap_err();

        assert!(error.completed().is_empty());
        assert!(matches!(
            error.failure(),
            CommitFailure::WriteExternal { path: failed, .. } if failed == &path
        ));
        assert_eq!(fs::read(occupied).unwrap(), b"appeared after discovery");
        assert_eq!(fs::read(document).unwrap(), b"old document");
    }

    #[test]
    fn stops_in_canonical_order_and_reports_only_completed_external_writes() {
        let a_destination = Path::new("out/a");
        let b_destination = Path::new("out/b");
        let c_destination = Path::new("out/c");
        let a = FragmentPath::try_from("a").unwrap();
        let b = FragmentPath::try_from("b").unwrap();
        let c = FragmentPath::try_from("c").unwrap();
        let external = [
            ExternalOutput {
                path: &a,
                destination: a_destination,
                bytes: b"a",
                write: ExternalWrite::CreateOrTruncate,
            },
            ExternalOutput {
                path: &b,
                destination: b_destination,
                bytes: b"b",
                write: ExternalWrite::CreateOrTruncate,
            },
            ExternalOutput {
                path: &c,
                destination: c_destination,
                bytes: b"c",
                write: ExternalWrite::CreateOrTruncate,
            },
        ];
        let mut filesystem = RecordingFilesystem {
            events: Vec::new(),
            fail_write: Some(b_destination.to_owned()),
        };

        let error = commit_with(&mut filesystem, &external, None).unwrap_err();

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
                .contains("after writing external fragment `a`")
        );
    }

    #[test]
    fn rejects_noncanonical_output_order_before_filesystem_effects() {
        let a = FragmentPath::try_from("a").unwrap();
        let b = FragmentPath::try_from("b").unwrap();
        let external = [
            ExternalOutput {
                path: &b,
                destination: Path::new("b"),
                bytes: b"b",
                write: ExternalWrite::CreateOrTruncate,
            },
            ExternalOutput {
                path: &a,
                destination: Path::new("a"),
                bytes: b"a",
                write: ExternalWrite::CreateOrTruncate,
            },
        ];
        let mut filesystem = RecordingFilesystem {
            events: Vec::new(),
            fail_write: None,
        };

        let error = commit_with(&mut filesystem, &external, None).unwrap_err();

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
                destination: Path::new("a"),
                bytes: b"a",
                write: ExternalWrite::CreateOrTruncate,
            },
            ExternalOutput {
                path: &b,
                destination: Path::new("b"),
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

    struct RecordingFilesystem {
        events: Vec<String>,
        fail_write: Option<PathBuf>,
    }

    impl RecordingFilesystem {
        fn record(&mut self, operation: &str, path: &Path) {
            self.events.push(format!("{operation}:{}", path.display()));
        }
    }

    impl CommitFilesystem for RecordingFilesystem {
        fn create_dir_all(&mut self, path: &Path) -> io::Result<()> {
            self.record("mkdir", path);
            Ok(())
        }

        fn write(&mut self, path: &Path, _bytes: &[u8], _write: ExternalWrite) -> io::Result<()> {
            self.record("write", path);
            if self.fail_write.as_deref() == Some(path) {
                return Err(io::Error::other("injected failure"));
            }
            Ok(())
        }
    }
}
