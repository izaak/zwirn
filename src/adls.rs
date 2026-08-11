//! Narrow, preservation-oriented access to source fields in Audulus documents.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::str::Utf8Error;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

const FILE_IDENTIFIER: &[u8; 4] = b"ADLS";
const HEADER_SIZE: usize = 8;
const MAX_REWRITTEN_ADLS_SIZE: usize = 1_usize << 31;
const U32_SIZE: usize = size_of::<u32>();
const VTABLE_HEADER_SIZE: usize = 2 * size_of::<u16>();
static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);

/// A patch-object index in one parsed document.
///
/// Handles are only meaningful with the [`Document`] that produced them. Audulus
/// may assign different indexes when it saves a document.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeHandle {
    index: u32,
    document: DocumentId,
}

impl NodeHandle {
    /// Returns this node's index in the document's patch-object pool.
    pub const fn index(self) -> u32 {
        self.index
    }
}

impl std::fmt::Debug for NodeHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("NodeHandle")
            .field(&self.index)
            .finish()
    }
}

impl std::fmt::Display for NodeHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.index.fmt(formatter)
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct DocumentId(u64);

impl DocumentId {
    fn fresh() -> Self {
        Self(NEXT_DOCUMENT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// A kind of Audulus node whose `f10` field contains source code.
///
/// Callers should derive language-specific behavior from this value rather than
/// from the presence or contents of the source field.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NodeKind {
    Shader,
    Canvas,
    Dsp,
    LyteDsp,
}

impl NodeKind {
    fn from_type_id(type_id: u32) -> Option<Self> {
        match type_id {
            74 => Some(Self::Shader),
            78 => Some(Self::Canvas),
            79 => Some(Self::Dsp),
            82 => Some(Self::LyteDsp),
            _ => None,
        }
    }
}

/// One present source field in a document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceNode<'a> {
    pub handle: NodeHandle,
    pub kind: NodeKind,
    pub source: &'a str,
}

/// A validated, borrowed view of the source-bearing part of an Audulus document.
#[derive(Debug)]
pub struct Document<'a> {
    bytes: &'a [u8],
    sources: Vec<SourceNode<'a>>,
    source_field_locations: Vec<usize>,
}

impl<'a> Document<'a> {
    /// Parses and validates every structure reached by the source-field view.
    ///
    /// Repeated patch-object references and aliased storage for an accessed
    /// table field are rejected because they cannot represent independently
    /// rewritable node fields while preserving unknown data. Shared vtables and
    /// shared string targets remain supported because rewriting does not mutate
    /// them.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::InvalidHeader);
        }
        if &bytes[4..8] != FILE_IDENTIFIER {
            return Err(Error::InvalidIdentifier);
        }

        let parser = Parser { bytes };
        let document_id = DocumentId::fresh();
        let mut layout = Layout::new();
        layout.add(0, HEADER_SIZE, RegionKind::Header, "ADLS header")?;
        let root_location = parser.follow_uoffset(0, U32_SIZE, "root table")?;
        let root = parser.table(root_location, "root table")?;
        layout.add_table(root, "root table")?;
        let pool_field = root
            .field(&parser, 0, U32_SIZE, U32_SIZE, "root f0")?
            .ok_or(Error::MissingPatchObjectPool)?;
        let pool_location = parser.follow_uoffset(pool_field, U32_SIZE, "patch-object vector")?;
        let object_count = parser.read_u32(pool_location, "patch-object vector")?;
        let object_count = usize::try_from(object_count)
            .map_err(|_| Error::malformed(pool_location, "patch-object vector"))?;
        if object_count == 0 {
            return Err(Error::malformed(pool_location, "patch-object vector"));
        }
        let elements_location = parser.checked_add(
            pool_location,
            U32_SIZE,
            pool_location,
            "patch-object vector",
        )?;
        let elements_size = object_count
            .checked_mul(U32_SIZE)
            .ok_or_else(|| Error::malformed(pool_location, "patch-object vector"))?;
        parser.require_range(elements_location, elements_size, "patch-object vector")?;
        let pool_end = elements_location
            .checked_add(elements_size)
            .ok_or_else(|| Error::malformed(pool_location, "patch-object vector"))?;
        layout.add(
            pool_location,
            pool_end,
            RegionKind::Vector,
            "patch-object vector",
        )?;

        let mut sources = Vec::new();
        let mut source_field_locations = Vec::new();
        let mut object_indexes = BTreeMap::new();

        for index in 0..object_count {
            let element_offset = index
                .checked_mul(U32_SIZE)
                .ok_or_else(|| Error::malformed(elements_location, "patch-object vector"))?;
            let element_location = parser.checked_add(
                elements_location,
                element_offset,
                elements_location,
                "patch-object vector",
            )?;
            let object_location =
                parser.follow_uoffset(element_location, U32_SIZE, "patch object")?;
            let pool_index = u32::try_from(index)
                .map_err(|_| Error::malformed(element_location, "patch-object index"))?;
            if let Some(first_index) = object_indexes.insert(object_location, pool_index) {
                return Err(Error::DuplicatePatchObjectTable {
                    first_index,
                    second_index: pool_index,
                });
            }
            let object = parser.table(object_location, "patch object")?;
            layout.add_table(object, "patch object")?;
            let type_id = match object.field(&parser, 0, U32_SIZE, U32_SIZE, "patch object f0")? {
                Some(location) => parser.read_u32(location, "patch object f0")?,
                None => 0,
            };
            if index == 0 && type_id != 0 {
                return Err(Error::malformed(object_location, "root patch object"));
            }
            let Some(kind) = NodeKind::from_type_id(type_id) else {
                continue;
            };
            let Some(source_field_location) =
                object.field(&parser, 10, U32_SIZE, U32_SIZE, "patch object f10")?
            else {
                continue;
            };

            let handle = NodeHandle {
                index: pool_index,
                document: document_id,
            };
            let source_location =
                parser.follow_uoffset(source_field_location, U32_SIZE, "source string")?;
            let source_length = parser.read_u32(source_location, "source string")?;
            let source_length = usize::try_from(source_length)
                .map_err(|_| Error::malformed(source_location, "source string"))?;
            let source_bytes_location =
                parser.checked_add(source_location, U32_SIZE, source_location, "source string")?;
            let terminator_location = parser.checked_add(
                source_bytes_location,
                source_length,
                source_location,
                "source string",
            )?;
            parser.require_range(source_bytes_location, source_length, "source string")?;
            let terminator = parser
                .bytes
                .get(terminator_location)
                .ok_or_else(|| Error::malformed(terminator_location, "source string terminator"))?;
            if *terminator != 0 {
                return Err(Error::malformed(
                    terminator_location,
                    "source string terminator",
                ));
            }
            let source_end = terminator_location
                .checked_add(1)
                .ok_or_else(|| Error::malformed(source_location, "source string"))?;
            layout.add(
                source_location,
                source_end,
                RegionKind::String,
                "source string",
            )?;
            let source_bytes =
                &parser.bytes[source_bytes_location..source_bytes_location + source_length];
            let source =
                std::str::from_utf8(source_bytes).map_err(|source| Error::InvalidSourceUtf8 {
                    node: handle,
                    source,
                })?;

            sources.push(SourceNode {
                handle,
                kind,
                source,
            });
            source_field_locations.push(source_field_location);
        }

        Ok(Self {
            bytes,
            sources,
            source_field_locations,
        })
    }

    /// Returns present source fields in patch-object vector (document) order.
    pub fn sources(&self) -> &[SourceNode<'a>] {
        &self.sources
    }

    /// Replaces a batch of present source fields.
    ///
    /// An empty batch, or a batch whose values are all unchanged, borrows the
    /// original bytes exactly. Replacement handles must come from [`Self::sources`]
    /// and may occur at most once in a batch.
    pub fn rewrite<'source>(
        &self,
        replacements: &[(NodeHandle, &'source str)],
    ) -> Result<Cow<'a, [u8]>, Error> {
        if replacements.is_empty() {
            return Ok(Cow::Borrowed(self.bytes));
        }
        let mut requested = vec![None; self.sources.len()];

        for &(handle, replacement) in replacements {
            let source_index = self
                .sources
                .binary_search_by_key(&handle.index(), |source| source.handle.index())
                .map_err(|_| Error::UnknownNodeHandle { node: handle })?;
            if self.sources[source_index].handle != handle {
                return Err(Error::UnknownNodeHandle { node: handle });
            }
            if requested[source_index].replace(replacement).is_some() {
                return Err(Error::DuplicateReplacement { node: handle });
            }
        }

        let mut changes = Vec::new();
        for (index, replacement) in requested.into_iter().enumerate() {
            let Some(replacement) = replacement else {
                continue;
            };
            if replacement == self.sources[index].source {
                continue;
            }
            let field_location = self.source_field_locations[index];
            let replacement_length =
                u32::try_from(replacement.len()).map_err(|_| Error::SizeLimitExceeded)?;
            changes.push((field_location, replacement, replacement_length));
        }

        if changes.is_empty() {
            return Ok(Cow::Borrowed(self.bytes));
        }

        let planned_size = plan_rewrite_size(
            self.bytes.len(),
            changes.iter().map(|(_, replacement, _)| replacement.len()),
        )?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(planned_size)
            .map_err(|_| Error::SizeLimitExceeded)?;
        output.extend_from_slice(self.bytes);
        for (field_location, replacement, replacement_length) in changes {
            let padding = (U32_SIZE - output.len() % U32_SIZE) % U32_SIZE;
            output.resize(output.len() + padding, 0);

            let string_location = output.len();
            let relative_offset = string_location
                .checked_sub(field_location)
                .and_then(|offset| u32::try_from(offset).ok())
                .ok_or(Error::SizeLimitExceeded)?;
            output.extend_from_slice(&replacement_length.to_le_bytes());
            output.extend_from_slice(replacement.as_bytes());
            output.push(0);
            output[field_location..field_location + U32_SIZE]
                .copy_from_slice(&relative_offset.to_le_bytes());
        }
        debug_assert_eq!(output.len(), planned_size);

        // Keep construction and validation coupled: a successful rewrite is
        // guaranteed to be consumable by this same narrow view.
        Document::parse(&output)?;
        Ok(Cow::Owned(output))
    }
}

/// Failures while parsing or rewriting the narrow ADLS source view.
#[derive(Debug, Error)]
pub enum Error {
    #[error("the input is shorter than the eight-byte ADLS header")]
    InvalidHeader,

    #[error("the input does not have the ADLS file identifier")]
    InvalidIdentifier,

    #[error("malformed {structure} at byte {offset}")]
    Malformed {
        offset: usize,
        structure: &'static str,
    },

    #[error("the root table has no patch-object pool in f0")]
    MissingPatchObjectPool,

    #[error("patch-object pool indexes {first_index} and {second_index} resolve to the same table")]
    DuplicatePatchObjectTable { first_index: u32, second_index: u32 },

    #[error("source for node {node} is not valid UTF-8")]
    InvalidSourceUtf8 {
        node: NodeHandle,
        #[source]
        source: Utf8Error,
    },

    #[error("node handle {node} is not a present source field in this document")]
    UnknownNodeHandle { node: NodeHandle },

    #[error("node handle {node} occurs more than once in the replacement batch")]
    DuplicateReplacement { node: NodeHandle },

    #[error("the rewritten ADLS buffer would exceed Zwirn's 2^31-byte size limit")]
    SizeLimitExceeded,
}

impl Error {
    fn malformed(offset: usize, structure: &'static str) -> Self {
        Self::Malformed { offset, structure }
    }
}

fn plan_rewrite_size(
    initial_size: usize,
    replacement_lengths: impl IntoIterator<Item = usize>,
) -> Result<usize, Error> {
    if initial_size > MAX_REWRITTEN_ADLS_SIZE {
        return Err(Error::SizeLimitExceeded);
    }

    let mut size = initial_size;
    for replacement_length in replacement_lengths {
        let padding = (U32_SIZE - size % U32_SIZE) % U32_SIZE;
        size = size
            .checked_add(padding)
            .and_then(|size| size.checked_add(U32_SIZE))
            .and_then(|size| size.checked_add(replacement_length))
            .and_then(|size| size.checked_add(1))
            .filter(|size| *size <= MAX_REWRITTEN_ADLS_SIZE)
            .ok_or(Error::SizeLimitExceeded)?;
    }
    Ok(size)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RegionKind {
    Header,
    Table,
    Vector,
    Vtable,
    String,
}

impl RegionKind {
    fn can_share(self) -> bool {
        matches!(self, Self::Vtable | Self::String)
    }
}

#[derive(Clone, Copy)]
struct Region {
    end: usize,
    kind: RegionKind,
}

struct Layout {
    regions: BTreeMap<usize, Region>,
}

impl Layout {
    fn new() -> Self {
        Self {
            regions: BTreeMap::new(),
        }
    }

    fn add_table(&mut self, table: Table, structure: &'static str) -> Result<(), Error> {
        let table_end = table
            .location
            .checked_add(table.object_length)
            .ok_or_else(|| Error::malformed(table.location, structure))?;
        self.add(table.location, table_end, RegionKind::Table, structure)?;
        let vtable_end = table
            .vtable_location
            .checked_add(table.vtable_length)
            .ok_or_else(|| Error::malformed(table.vtable_location, structure))?;
        self.add(
            table.vtable_location,
            vtable_end,
            RegionKind::Vtable,
            structure,
        )
    }

    fn add(
        &mut self,
        start: usize,
        end: usize,
        kind: RegionKind,
        structure: &'static str,
    ) -> Result<(), Error> {
        if end <= start {
            return Err(Error::malformed(start, structure));
        }
        if let Some(existing) = self.regions.get(&start) {
            if existing.end == end && existing.kind == kind && kind.can_share() {
                return Ok(());
            }
            return Err(Error::malformed(start, structure));
        }
        if self
            .regions
            .range(..start)
            .next_back()
            .is_some_and(|(_, previous)| previous.end > start)
        {
            return Err(Error::malformed(start, structure));
        }
        if self
            .regions
            .range(start..)
            .next()
            .is_some_and(|(next_start, _)| *next_start < end)
        {
            return Err(Error::malformed(start, structure));
        }
        self.regions.insert(start, Region { end, kind });
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Table {
    location: usize,
    vtable_location: usize,
    vtable_length: usize,
    object_length: usize,
}

impl Table {
    fn field(
        self,
        parser: &Parser<'_>,
        field_index: usize,
        width: usize,
        alignment: usize,
        structure: &'static str,
    ) -> Result<Option<usize>, Error> {
        let entry_offset = field_index
            .checked_mul(size_of::<u16>())
            .and_then(|offset| VTABLE_HEADER_SIZE.checked_add(offset))
            .ok_or_else(|| Error::malformed(self.vtable_location, structure))?;
        let entry_end = entry_offset
            .checked_add(size_of::<u16>())
            .ok_or_else(|| Error::malformed(self.vtable_location, structure))?;
        if entry_end > self.vtable_length {
            return Ok(None);
        }
        let entry_location = parser.checked_add(
            self.vtable_location,
            entry_offset,
            self.vtable_location,
            structure,
        )?;
        let field_offset = usize::from(parser.read_u16(entry_location, structure)?);
        if field_offset == 0 {
            return Ok(None);
        }
        let field_end = field_offset
            .checked_add(width)
            .ok_or_else(|| Error::malformed(entry_location, structure))?;
        if field_end > self.object_length {
            return Err(Error::malformed(entry_location, structure));
        }

        // The view depends on each accessed field having one logical identity;
        // f10 is also rewritten in place. Reject shared inline storage.
        for other_entry_offset in (VTABLE_HEADER_SIZE..self.vtable_length).step_by(size_of::<u16>())
        {
            if other_entry_offset == entry_offset {
                continue;
            }
            let other_entry_location = parser.checked_add(
                self.vtable_location,
                other_entry_offset,
                self.vtable_location,
                structure,
            )?;
            let other_field_offset = usize::from(parser.read_u16(other_entry_location, structure)?);
            if (field_offset..field_end).contains(&other_field_offset) {
                return Err(Error::malformed(other_entry_location, structure));
            }
        }
        let field_location =
            parser.checked_add(self.location, field_offset, entry_location, structure)?;
        if !field_location.is_multiple_of(alignment) {
            return Err(Error::malformed(field_location, structure));
        }
        parser.require_range(field_location, width, structure)?;
        Ok(Some(field_location))
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
}

impl Parser<'_> {
    fn table(&self, location: usize, structure: &'static str) -> Result<Table, Error> {
        if !location.is_multiple_of(U32_SIZE) {
            return Err(Error::malformed(location, structure));
        }
        let vtable_offset = self.read_i32(location, structure)?;
        if vtable_offset == 0 {
            return Err(Error::malformed(location, structure));
        }
        let vtable_distance = usize::try_from(vtable_offset.unsigned_abs())
            .map_err(|_| Error::malformed(location, structure))?;
        let vtable_location = if vtable_offset.is_positive() {
            location
                .checked_sub(vtable_distance)
                .ok_or_else(|| Error::malformed(location, structure))?
        } else {
            location
                .checked_add(vtable_distance)
                .ok_or_else(|| Error::malformed(location, structure))?
        };
        if !vtable_location.is_multiple_of(size_of::<u16>()) {
            return Err(Error::malformed(vtable_location, structure));
        }

        let vtable_length = usize::from(self.read_u16(vtable_location, structure)?);
        let object_length_location = self.checked_add(
            vtable_location,
            size_of::<u16>(),
            vtable_location,
            structure,
        )?;
        let object_length = usize::from(self.read_u16(object_length_location, structure)?);
        if vtable_length < VTABLE_HEADER_SIZE || !vtable_length.is_multiple_of(size_of::<u16>()) {
            return Err(Error::malformed(vtable_location, structure));
        }
        if object_length < size_of::<i32>() {
            return Err(Error::malformed(location, structure));
        }
        self.require_range(vtable_location, vtable_length, structure)?;
        self.require_range(location, object_length, structure)?;
        for entry_offset in (VTABLE_HEADER_SIZE..vtable_length).step_by(size_of::<u16>()) {
            let entry_location =
                self.checked_add(vtable_location, entry_offset, vtable_location, structure)?;
            let field_offset = usize::from(self.read_u16(entry_location, structure)?);
            if field_offset != 0
                && (field_offset < size_of::<i32>() || field_offset >= object_length)
            {
                return Err(Error::malformed(entry_location, structure));
            }
        }

        Ok(Table {
            location,
            vtable_location,
            vtable_length,
            object_length,
        })
    }

    fn follow_uoffset(
        &self,
        location: usize,
        alignment: usize,
        structure: &'static str,
    ) -> Result<usize, Error> {
        if !location.is_multiple_of(U32_SIZE) {
            return Err(Error::malformed(location, structure));
        }
        let relative = usize::try_from(self.read_u32(location, structure)?)
            .map_err(|_| Error::malformed(location, structure))?;
        if relative == 0 {
            return Err(Error::malformed(location, structure));
        }
        let target = self.checked_add(location, relative, location, structure)?;
        if !target.is_multiple_of(alignment) || target >= self.bytes.len() {
            return Err(Error::malformed(location, structure));
        }
        Ok(target)
    }

    fn read_u16(&self, location: usize, structure: &'static str) -> Result<u16, Error> {
        let bytes = self.array::<2>(location, structure)?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn read_u32(&self, location: usize, structure: &'static str) -> Result<u32, Error> {
        let bytes = self.array::<4>(location, structure)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_i32(&self, location: usize, structure: &'static str) -> Result<i32, Error> {
        let bytes = self.array::<4>(location, structure)?;
        Ok(i32::from_le_bytes(bytes))
    }

    fn array<const SIZE: usize>(
        &self,
        location: usize,
        structure: &'static str,
    ) -> Result<[u8; SIZE], Error> {
        let end = location
            .checked_add(SIZE)
            .ok_or_else(|| Error::malformed(location, structure))?;
        let bytes = self
            .bytes
            .get(location..end)
            .ok_or_else(|| Error::malformed(location, structure))?;
        let mut array = [0; SIZE];
        array.copy_from_slice(bytes);
        Ok(array)
    }

    fn checked_add(
        &self,
        left: usize,
        right: usize,
        error_location: usize,
        structure: &'static str,
    ) -> Result<usize, Error> {
        left.checked_add(right)
            .ok_or_else(|| Error::malformed(error_location, structure))
    }

    fn require_range(
        &self,
        location: usize,
        length: usize,
        structure: &'static str,
    ) -> Result<(), Error> {
        let end = location
            .checked_add(length)
            .ok_or_else(|| Error::malformed(location, structure))?;
        if end > self.bytes.len() {
            return Err(Error::malformed(location, structure));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, MAX_REWRITTEN_ADLS_SIZE, plan_rewrite_size};

    #[test]
    fn plans_the_rewrite_size_at_the_limit_without_allocating_the_buffer() {
        let aligned_initial_size = MAX_REWRITTEN_ADLS_SIZE - 8;

        assert_eq!(
            plan_rewrite_size(aligned_initial_size, [3]).unwrap(),
            MAX_REWRITTEN_ADLS_SIZE
        );
        assert!(matches!(
            plan_rewrite_size(aligned_initial_size, [4]),
            Err(Error::SizeLimitExceeded)
        ));
    }
}
