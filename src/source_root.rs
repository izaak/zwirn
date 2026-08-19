//! Capability-anchored access to fragment targets beneath one source root.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::{Dir, File, Metadata, MetadataExt, OpenOptions};

use crate::fragment::FragmentPath;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    pub(crate) fn new(device: u64, inode: u64) -> Self {
        Self { device, inode }
    }

    fn from_metadata(metadata: &Metadata) -> Self {
        Self::new(metadata.dev(), metadata.ino())
    }
}

#[derive(Debug)]
pub(crate) struct SourceRoot {
    path: PathBuf,
    directory: Dir,
}

impl SourceRoot {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let directory = Dir::open_ambient_dir(path, ambient_authority())?;
        Ok(Self {
            path: path.to_path_buf(),
            directory,
        })
    }

    pub(crate) fn named_path(&self, path: &Path) -> PathBuf {
        self.path.join(path)
    }

    pub(crate) fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        self.directory.metadata(path)
    }

    pub(crate) fn open_target(&self, path: &FragmentPath) -> io::Result<File> {
        self.directory.open(relative_path(path))
    }

    pub(crate) fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.directory.create_dir_all(path)
    }

    pub(crate) fn create_target(
        &self,
        path: &FragmentPath,
        bytes: &[u8],
        absent_targets: &[&FragmentPath],
    ) -> io::Result<Option<FragmentPath>> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = self.directory.open_with(relative_path(path), &options)?;
        // The exclusive creation handle is authoritative for the new file.
        // Comparison probes remain capability-relative so alias detection does
        // not weaken source-root containment.
        let identity = FileIdentity::from_metadata(&file.metadata()?);
        file.write_all(bytes)?;
        Ok(absent_targets
            .iter()
            .copied()
            .find(|candidate| {
                *candidate != path
                    && self
                        .target_identity(candidate)
                        .is_ok_and(|candidate| candidate == identity)
            })
            .cloned())
    }

    pub(crate) fn replace_target(&self, path: &FragmentPath, bytes: &[u8]) -> io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        self.write_target(path, bytes, &options)
    }

    fn target_identity(&self, path: &FragmentPath) -> io::Result<FileIdentity> {
        self.metadata(relative_path(path))
            .map(|metadata| FileIdentity::from_metadata(&metadata))
    }

    fn write_target(
        &self,
        path: &FragmentPath,
        bytes: &[u8],
        options: &OpenOptions,
    ) -> io::Result<()> {
        self.directory
            .open_with(relative_path(path), options)?
            .write_all(bytes)
    }
}

pub(crate) fn relative_path(path: &FragmentPath) -> &Path {
    Path::new(path.as_str())
}
