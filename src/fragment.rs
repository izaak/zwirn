//! Canonical fragment values and surgical access to markers in embedded source.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::str::FromStr;

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::adls::NodeKind;

/// A canonical source-root-relative fragment path.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FragmentPath(String);

impl FragmentPath {
    /// Returns the canonical `/`-separated representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for FragmentPath {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for FragmentPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for FragmentPath {
    type Err = FragmentPathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl TryFrom<&str> for FragmentPath {
    type Error = FragmentPathError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_owned())
    }
}

impl TryFrom<String> for FragmentPath {
    type Error = FragmentPathError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_fragment_path(&value)?;
        Ok(Self(value))
    }
}

fn validate_fragment_path(value: &str) -> Result<(), FragmentPathError> {
    if value.is_empty() {
        return Err(FragmentPathError::Empty);
    }
    if value.starts_with('/') {
        return Err(FragmentPathError::LeadingSlash);
    }
    if value.ends_with('/') {
        return Err(FragmentPathError::TrailingSlash);
    }
    if value.contains('\\') {
        return Err(FragmentPathError::Backslash);
    }
    if value.chars().any(char::is_whitespace) {
        return Err(FragmentPathError::Whitespace);
    }
    for segment in value.split('/') {
        match segment {
            "" => return Err(FragmentPathError::EmptySegment),
            "." => return Err(FragmentPathError::CurrentDirectorySegment),
            ".." => return Err(FragmentPathError::ParentDirectorySegment),
            _ => {}
        }
    }
    Ok(())
}

/// Why a string is not a canonical fragment path.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum FragmentPathError {
    #[error("a fragment path is empty")]
    Empty,

    #[error("a fragment path begins with `/`")]
    LeadingSlash,

    #[error("a fragment path ends with `/`")]
    TrailingSlash,

    #[error("a fragment path contains a backslash")]
    Backslash,

    #[error("a fragment path contains whitespace")]
    Whitespace,

    #[error("a fragment path contains an empty segment")]
    EmptySegment,

    #[error("a fragment path contains a `.` segment")]
    CurrentDirectorySegment,

    #[error("a fragment path contains a `..` segment")]
    ParentDirectorySegment,
}

/// The first eight bytes of a SHA-256 digest of canonical source.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BaselineHash([u8; 8]);

impl BaselineHash {
    /// Computes the baseline hash of canonical source.
    pub fn from_source(source: &CanonicalSource) -> Self {
        let digest = Sha256::digest(source.as_str().as_bytes());
        let mut prefix = [0; 8];
        prefix.copy_from_slice(&digest[..8]);
        Self(prefix)
    }

    /// Returns the digest prefix in digest byte order.
    pub const fn as_bytes(&self) -> &[u8; 8] {
        &self.0
    }
}

impl fmt::Debug for BaselineHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("BaselineHash")
            .field(&self.to_string())
            .finish()
    }
}

impl fmt::Display for BaselineHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for BaselineHash {
    type Err = BaselineHashError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 16 {
            return Err(BaselineHashError::InvalidLength {
                actual: value.len(),
            });
        }

        let mut bytes = [0; 8];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = lowercase_hex_value(pair[0])
                .ok_or(BaselineHashError::InvalidCharacter { index: index * 2 })?;
            let low = lowercase_hex_value(pair[1]).ok_or(BaselineHashError::InvalidCharacter {
                index: index * 2 + 1,
            })?;
            bytes[index] = high << 4 | low;
        }
        Ok(Self(bytes))
    }
}

impl TryFrom<&str> for BaselineHash {
    type Error = BaselineHashError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

fn lowercase_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Why a string is not a canonical baseline hash.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BaselineHashError {
    #[error("a baseline hash has {actual} bytes rather than 16")]
    InvalidLength { actual: usize },

    #[error("a baseline hash has a non-lowercase-hexadecimal byte at index {index}")]
    InvalidCharacter { index: usize },
}

/// UTF-8 fragment source in its canonical transferred representation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CanonicalSource(String);

impl CanonicalSource {
    /// Returns the canonical source text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_validated(value: &str) -> Self {
        debug_assert!(!value.starts_with('\u{feff}'));
        Self(canonicalize_source(value.to_owned()))
    }
}

impl AsRef<str> for CanonicalSource {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl TryFrom<&str> for CanonicalSource {
    type Error = CanonicalSourceError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_owned())
    }
}

impl TryFrom<String> for CanonicalSource {
    type Error = CanonicalSourceError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.starts_with('\u{feff}') {
            return Err(CanonicalSourceError::ByteOrderMark);
        }

        Ok(Self(canonicalize_source(value)))
    }
}

fn canonicalize_source(mut value: String) -> String {
    if value.contains('\r') {
        let mut normalized = String::with_capacity(value.len());
        let bytes = value.as_bytes();
        let mut segment_start = 0;
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'\r' {
                normalized.push_str(&value[segment_start..index]);
                normalized.push('\n');
                index += 1;
                if bytes.get(index) == Some(&b'\n') {
                    index += 1;
                }
                segment_start = index;
            } else {
                index += 1;
            }
        }
        normalized.push_str(&value[segment_start..]);
        value = normalized;
    }

    if !value.is_empty() && !value.ends_with('\n') {
        value.push('\n');
    }
    value
}

/// Why source cannot be represented canonically.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CanonicalSourceError {
    #[error("fragment source begins with a UTF-8 byte-order mark")]
    ByteOrderMark,
}

/// One discovered fragment in embedded source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fragment<'a> {
    pub path: &'a FragmentPath,
    pub baseline: Option<BaselineHash>,
    pub source: &'a str,
}

/// A parsed, borrowed view of the fragments in one source-bearing node.
#[derive(Debug)]
pub struct ParsedSource<'a> {
    source: &'a str,
    fragments: Vec<ParsedFragment>,
}

#[derive(Debug)]
struct ParsedFragment {
    path: FragmentPath,
    baseline: Option<BaselineHash>,
    source_span: Range<usize>,
    hash_location: HashLocation,
}

#[derive(Debug)]
enum HashLocation {
    Present(Range<usize>),
    Missing { insertion: usize },
}

impl<'a> ParsedSource<'a> {
    /// Parses and structurally validates all Zwirn markers in one node source.
    pub fn parse(kind: NodeKind, source: &'a str) -> Result<Self, ParseError> {
        let comment = marker_comment(kind);
        let mut fragments = Vec::new();
        let mut open: Option<OpenFragment> = None;

        for line in PhysicalLines::new(source) {
            let Some(marker) = parse_marker(comment, line)? else {
                continue;
            };

            match marker {
                Marker::Open { path } => {
                    if let Some(open) = &open {
                        return Err(ParseError::NestedOpening {
                            line: line.number,
                            opening_line: open.line,
                        });
                    }
                    open = Some(OpenFragment {
                        path,
                        line: line.number,
                        source_start: line.end,
                    });
                }
                Marker::Close {
                    path,
                    baseline,
                    hash_location,
                } => {
                    let Some(opened) = open.take() else {
                        return Err(ParseError::OrphanedClosing { line: line.number });
                    };
                    if path != opened.path {
                        return Err(ParseError::MismatchedClosingPath {
                            line: line.number,
                            opening_line: opened.line,
                            expected: opened.path,
                            found: path,
                        });
                    }

                    let source_span = opened.source_start..line.start;
                    if source[source_span.clone()].starts_with('\u{feff}') {
                        return Err(ParseError::InvalidFragmentSource {
                            line: opened.line + 1,
                            path: opened.path,
                            source: CanonicalSourceError::ByteOrderMark,
                        });
                    }
                    fragments.push(ParsedFragment {
                        path: opened.path,
                        baseline,
                        source_span,
                        hash_location,
                    });
                }
            }
        }

        if let Some(open) = open {
            return Err(ParseError::Unterminated {
                line: open.line,
                path: open.path,
            });
        }

        Ok(Self { source, fragments })
    }

    /// Iterates fragments in embedded source order.
    pub fn fragments(
        &self,
    ) -> impl ExactSizeIterator<Item = Fragment<'_>> + DoubleEndedIterator + '_ {
        self.fragments.iter().map(|fragment| Fragment {
            path: &fragment.path,
            baseline: fragment.baseline,
            source: &self.source[fragment.source_span.clone()],
        })
    }

    /// Applies a batch of source and baseline updates surgically.
    pub fn rewrite<'update>(
        &self,
        updates: &[FragmentUpdate<'update>],
    ) -> Result<Cow<'a, str>, RewriteError> {
        if updates.is_empty() {
            return Ok(Cow::Borrowed(self.source));
        }

        let mut requested = BTreeMap::new();
        for update in updates {
            let (path, source) = match update {
                FragmentUpdate::Record { path } => (*path, RequestedSource::Current),
                FragmentUpdate::Replace { path, source } => {
                    (*path, RequestedSource::Replacement(source))
                }
            };
            if requested.insert(path, source).is_some() {
                return Err(RewriteError::DuplicateUpdate { path: path.clone() });
            }
        }

        for path in requested.keys() {
            let mut matches = self
                .fragments
                .iter()
                .filter(|fragment| &fragment.path == *path);
            if matches.next().is_none() {
                return Err(RewriteError::UnknownPath {
                    path: (*path).clone(),
                });
            }
            if matches.next().is_some() {
                return Err(RewriteError::AmbiguousPath {
                    path: (*path).clone(),
                });
            }
        }

        let mut edits = Vec::new();
        for fragment in &self.fragments {
            let Some(requested_source) = requested.get(&fragment.path) else {
                continue;
            };

            let hash = match requested_source {
                RequestedSource::Current => {
                    let current =
                        CanonicalSource::from_validated(&self.source[fragment.source_span.clone()]);
                    BaselineHash::from_source(&current)
                }
                RequestedSource::Replacement(source) => {
                    if &self.source[fragment.source_span.clone()] != source.as_str() {
                        edits.push(TextEdit {
                            range: fragment.source_span.clone(),
                            replacement: Cow::Borrowed(source.as_str()),
                        });
                    }
                    BaselineHash::from_source(source)
                }
            };

            if fragment.baseline != Some(hash) {
                let (range, replacement) = match &fragment.hash_location {
                    HashLocation::Present(range) => (range.clone(), Cow::Owned(hash.to_string())),
                    HashLocation::Missing { insertion } => {
                        (*insertion..*insertion, Cow::Owned(format!(" {hash}")))
                    }
                };
                edits.push(TextEdit { range, replacement });
            }
        }

        if edits.is_empty() {
            return Ok(Cow::Borrowed(self.source));
        }

        let final_length = edits.iter().try_fold(self.source.len(), |length, edit| {
            length
                .checked_sub(edit.range.len())
                .and_then(|length| length.checked_add(edit.replacement.len()))
                .ok_or(RewriteError::SourceTooLarge)
        })?;
        let mut rewritten = String::new();
        rewritten
            .try_reserve_exact(final_length)
            .map_err(|_| RewriteError::AllocationFailed)?;

        let mut cursor = 0;
        for edit in edits {
            debug_assert!(cursor <= edit.range.start);
            rewritten.push_str(&self.source[cursor..edit.range.start]);
            rewritten.push_str(&edit.replacement);
            cursor = edit.range.end;
        }
        rewritten.push_str(&self.source[cursor..]);
        debug_assert_eq!(rewritten.len(), final_length);
        Ok(Cow::Owned(rewritten))
    }
}

fn marker_comment(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Canvas | NodeKind::Dsp => "--",
        NodeKind::Shader | NodeKind::LyteDsp => "//",
    }
}

#[derive(Debug)]
struct OpenFragment {
    path: FragmentPath,
    line: usize,
    source_start: usize,
}

#[derive(Debug)]
enum Marker {
    Open {
        path: FragmentPath,
    },
    Close {
        path: FragmentPath,
        baseline: Option<BaselineHash>,
        hash_location: HashLocation,
    },
}

fn parse_marker(comment: &str, line: PhysicalLine<'_>) -> Result<Option<Marker>, ParseError> {
    let bytes = line.content.as_bytes();
    let mut position = skip_horizontal(bytes, 0);
    if !bytes[position..].starts_with(comment.as_bytes()) {
        return Ok(None);
    }
    position += comment.len();

    let separator = position;
    position = skip_horizontal(bytes, position);
    if position == separator {
        return Ok(None);
    }

    let marker_start = position;
    position = token_end(bytes, position);
    let marker_token = &line.content[marker_start..position];
    let opening = match marker_token {
        "@{" => true,
        "@}" => false,
        _ => return Ok(None),
    };

    let separator = position;
    position = skip_horizontal(bytes, position);
    if position == separator || position == bytes.len() {
        return Err(ParseError::MissingPath { line: line.number });
    }

    let path_start = position;
    position = token_end(bytes, position);
    let path_end = position;
    let path = FragmentPath::try_from(&line.content[path_start..path_end]).map_err(|source| {
        ParseError::InvalidPath {
            line: line.number,
            source,
        }
    })?;

    position = skip_horizontal(bytes, position);
    if opening {
        if position != bytes.len() {
            return Err(ParseError::UnexpectedTokens { line: line.number });
        }
        return Ok(Some(Marker::Open { path }));
    }

    if position == bytes.len() {
        return Ok(Some(Marker::Close {
            path,
            baseline: None,
            hash_location: HashLocation::Missing {
                insertion: line.start + path_end,
            },
        }));
    }

    let hash_start = position;
    position = token_end(bytes, position);
    let hash_end = position;
    let baseline =
        BaselineHash::try_from(&line.content[hash_start..hash_end]).map_err(|source| {
            ParseError::InvalidHash {
                line: line.number,
                source,
            }
        })?;
    position = skip_horizontal(bytes, position);
    if position != bytes.len() {
        return Err(ParseError::UnexpectedTokens { line: line.number });
    }

    Ok(Some(Marker::Close {
        path,
        baseline: Some(baseline),
        hash_location: HashLocation::Present(line.start + hash_start..line.start + hash_end),
    }))
}

fn skip_horizontal(bytes: &[u8], mut position: usize) -> usize {
    while matches!(bytes.get(position), Some(b' ' | b'\t')) {
        position += 1;
    }
    position
}

fn token_end(bytes: &[u8], mut position: usize) -> usize {
    while position < bytes.len() && !matches!(bytes[position], b' ' | b'\t') {
        position += 1;
    }
    position
}

#[derive(Clone, Copy)]
struct PhysicalLine<'a> {
    number: usize,
    start: usize,
    end: usize,
    content: &'a str,
}

struct PhysicalLines<'a> {
    source: &'a str,
    next: usize,
    number: usize,
}

impl<'a> PhysicalLines<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            next: 0,
            number: 1,
        }
    }
}

impl<'a> Iterator for PhysicalLines<'a> {
    type Item = PhysicalLine<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.source.len() {
            return None;
        }

        let start = self.next;
        let bytes = self.source.as_bytes();
        let mut content_end = start;
        while content_end < bytes.len() && !matches!(bytes[content_end], b'\r' | b'\n') {
            content_end += 1;
        }

        let end = match bytes.get(content_end) {
            Some(b'\r') if bytes.get(content_end + 1) == Some(&b'\n') => content_end + 2,
            Some(b'\r' | b'\n') => content_end + 1,
            _ => content_end,
        };
        self.next = end;
        let number = self.number;
        self.number += 1;
        Some(PhysicalLine {
            number,
            start,
            end,
            content: &self.source[start..content_end],
        })
    }
}

/// A requested change to one parsed fragment.
#[derive(Clone, Copy, Debug)]
pub enum FragmentUpdate<'a> {
    /// Retain exact embedded source and record its canonical hash.
    Record { path: &'a FragmentPath },

    /// Replace embedded source and record the replacement's hash.
    Replace {
        path: &'a FragmentPath,
        source: &'a CanonicalSource,
    },
}

#[derive(Clone, Copy)]
enum RequestedSource<'a> {
    Current,
    Replacement(&'a CanonicalSource),
}

struct TextEdit<'a> {
    range: Range<usize>,
    replacement: Cow<'a, str>,
}

/// A structural marker error in one node source.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum ParseError {
    #[error("marker on line {line} has no fragment path")]
    MissingPath { line: usize },

    #[error("marker on line {line} contains unexpected tokens")]
    UnexpectedTokens { line: usize },

    #[error("marker on line {line} has an invalid fragment path")]
    InvalidPath {
        line: usize,
        #[source]
        source: FragmentPathError,
    },

    #[error("closing marker on line {line} has an invalid baseline hash")]
    InvalidHash {
        line: usize,
        #[source]
        source: BaselineHashError,
    },

    #[error(
        "opening marker on line {line} is nested inside the fragment opened on line {opening_line}"
    )]
    NestedOpening { line: usize, opening_line: usize },

    #[error("closing marker on line {line} has no opening marker")]
    OrphanedClosing { line: usize },

    #[error(
        "closing marker on line {line} does not match the fragment opened on line {opening_line}: expected `{expected}`, found `{found}`"
    )]
    MismatchedClosingPath {
        line: usize,
        opening_line: usize,
        expected: FragmentPath,
        found: FragmentPath,
    },

    #[error("fragment `{path}` opened on line {line} is unterminated")]
    Unterminated { line: usize, path: FragmentPath },

    #[error("fragment `{path}` source beginning on line {line} is invalid")]
    InvalidFragmentSource {
        line: usize,
        path: FragmentPath,
        #[source]
        source: CanonicalSourceError,
    },
}

/// A failure while applying fragment updates.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum RewriteError {
    #[error("fragment path `{path}` occurs more than once in the update batch")]
    DuplicateUpdate { path: FragmentPath },

    #[error("fragment path `{path}` does not occur in this parsed source")]
    UnknownPath { path: FragmentPath },

    #[error("fragment path `{path}` occurs more than once in this parsed source")]
    AmbiguousPath { path: FragmentPath },

    #[error("rewritten node source would exceed Rust string size limits")]
    SourceTooLarge,

    #[error("memory could not be reserved for rewritten node source")]
    AllocationFailed,
}
