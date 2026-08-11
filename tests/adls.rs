use std::borrow::Cow;

use zwirn::adls::{Document, Error, NodeKind};

const EMPTY: &[u8] = include_bytes!("fixtures/empty.audulus4");
const REPRESENTATIVE: &[u8] = include_bytes!("fixtures/representative.audulus4");
const SOURCE_TYPES: &[u8] = include_bytes!("fixtures/source-types.audulus4");

#[test]
fn reads_present_source_fields_in_document_order() {
    let document = Document::parse(SOURCE_TYPES).expect("authored fixture should parse");
    let summary = document
        .sources()
        .iter()
        .map(|node| (node.handle.index(), node.kind, node.source.len()))
        .collect::<Vec<_>>();

    assert_eq!(
        summary,
        [
            (2, NodeKind::Dsp, 122),
            (3, NodeKind::LyteDsp, 120),
            (5, NodeKind::Canvas, 153),
            (6, NodeKind::Shader, 232),
        ]
    );
    assert!(document.sources()[0].source.contains("function process"));
    assert!(document.sources()[1].source.contains("process {"));
    assert!(document.sources()[2].source.contains("fill_circle"));
    assert!(document.sources()[3].source.contains("outColor"));

    // The source-types fixture also contains a Text f10 and supported nodes with
    // absent f10 fields; neither belongs to this view.
    assert!(Document::parse(EMPTY).unwrap().sources().is_empty());
}

#[test]
fn node_kind_keeps_non_source_f10_outside_the_view() {
    let mut invalid_text = SOURCE_TYPES.to_vec();
    invalid_text[0x730] = 0xff;

    let document = Document::parse(&invalid_text).expect("Text f10 is not a source field");
    assert_eq!(
        document
            .sources()
            .iter()
            .map(|node| node.handle.index())
            .collect::<Vec<_>>(),
        [2, 3, 5, 6]
    );
}

#[test]
fn yields_a_present_zero_length_source_field() {
    let mut present_empty = SOURCE_TYPES.to_vec();
    present_empty[0x604..0x608].copy_from_slice(&0_u32.to_le_bytes());
    present_empty[0x608] = 0;

    let document = Document::parse(&present_empty).expect("empty string remains present");
    assert_eq!(document.sources()[0].handle.index(), 2);
    assert_eq!(document.sources()[0].kind, NodeKind::Dsp);
    assert_eq!(document.sources()[0].source, "");
    assert_eq!(document.sources().len(), 4);
}

#[test]
fn rewrites_empty_shorter_and_longer_utf8_sources_as_one_batch() {
    let document = Document::parse(SOURCE_TYPES).unwrap();
    let original_shader = document.sources()[3].source;
    let long_canvas = "-- arbitrary UTF-8, including NUL: 雪 😀\0\n".repeat(6);
    assert!(long_canvas.len() > document.sources()[2].source.len());

    let replacements = [
        (document.sources()[0].handle, ""),
        (document.sources()[1].handle, "λ"),
        (document.sources()[2].handle, long_canvas.as_str()),
    ];
    let rewritten = document.rewrite(&replacements).unwrap();
    let Cow::Owned(rewritten) = rewritten else {
        panic!("an effective replacement must produce owned bytes");
    };

    // Appending new strings preserves every original byte except the selected
    // inline f10 offset words, including all unknown serialized fields.
    const CHANGED_SLOTS: [usize; 3] = [0x05c0, 0x0468, 0x02b0];
    for (offset, (&before, &after)) in SOURCE_TYPES.iter().zip(&rewritten).enumerate() {
        if !CHANGED_SLOTS
            .iter()
            .any(|slot| (*slot..*slot + 4).contains(&offset))
        {
            assert_eq!(before, after, "unexpected prefix change at {offset:#x}");
        }
    }
    for slot in CHANGED_SLOTS {
        assert_ne!(&SOURCE_TYPES[slot..slot + 4], &rewritten[slot..slot + 4]);
    }

    let reparsed = Document::parse(&rewritten).expect("rewritten bytes should parse");
    assert_eq!(reparsed.sources().len(), 4);
    assert_eq!(reparsed.sources()[0].source, "");
    assert_eq!(reparsed.sources()[1].source, "λ");
    assert_eq!(reparsed.sources()[2].source, long_canvas);
    assert_eq!(reparsed.sources()[3].source, original_shader);
    assert_eq!(
        reparsed
            .sources()
            .iter()
            .map(|node| node.kind)
            .collect::<Vec<_>>(),
        [
            NodeKind::Dsp,
            NodeKind::LyteDsp,
            NodeKind::Canvas,
            NodeKind::Shader,
        ]
    );
}

#[test]
fn rewriting_one_shared_string_reference_leaves_the_other_unchanged() {
    let mut shared_string = SOURCE_TYPES.to_vec();
    shared_string[0x468..0x46c].copy_from_slice(&0x19c_u32.to_le_bytes());
    let document = Document::parse(&shared_string).expect("shared strings are valid");
    assert_eq!(document.sources()[0].source, document.sources()[1].source);

    let original_shared_source = document.sources()[1].source;
    let rewritten = document
        .rewrite(&[(document.sources()[0].handle, "selected only")])
        .unwrap();
    let reparsed = Document::parse(rewritten.as_ref()).unwrap();

    assert_eq!(reparsed.sources()[0].source, "selected only");
    assert_eq!(reparsed.sources()[1].source, original_shared_source);
}

#[test]
fn preserves_input_bytes_for_empty_and_unchanged_batches() {
    let document = Document::parse(REPRESENTATIVE).unwrap();

    let untouched = document.rewrite(&[]).unwrap();
    assert!(matches!(untouched, Cow::Borrowed(_)));
    assert_eq!(untouched.as_ref(), REPRESENTATIVE);

    let unchanged = document
        .sources()
        .iter()
        .map(|node| (node.handle, node.source))
        .collect::<Vec<_>>();
    let untouched = document.rewrite(&unchanged).unwrap();
    assert!(matches!(untouched, Cow::Borrowed(_)));
    assert_eq!(untouched.as_ref(), REPRESENTATIVE);
}

#[test]
fn rejects_malformed_reached_structures_and_invalid_source_utf8() {
    let malformed_cases: &[(usize, &[u8])] = &[
        (0, &u32::MAX.to_le_bytes()),
        (0x20, &u32::MAX.to_le_bytes()),
        (0x54, &0_u32.to_le_bytes()),
        (0x54, &u32::MAX.to_le_bytes()),
        (0x58, &0x84_u32.to_le_bytes()),
        (0x60, &u32::MAX.to_le_bytes()),
        (0x68, &0x534_u32.to_le_bytes()),
        (0x532, &u16::MAX.to_le_bytes()),
        (0x532, &0x24_u16.to_le_bytes()),
        (0x604, &u32::MAX.to_le_bytes()),
        (0x682, &[1]),
    ];

    for &(offset, replacement) in malformed_cases {
        let mut malformed = SOURCE_TYPES.to_vec();
        malformed[offset..offset + replacement.len()].copy_from_slice(replacement);
        let error = Document::parse(&malformed).expect_err("mutation should be rejected");
        assert!(
            matches!(error, Error::Malformed { .. }),
            "unexpected error for {offset:#x}: {error}"
        );
    }

    // A second table nested inside object[2]'s inline span would let its fields
    // alias source storage even though the table and vtable are individually
    // readable.
    let mut overlapping_tables = SOURCE_TYPES.to_vec();
    overlapping_tables[0x64..0x68].copy_from_slice(&0x558_u32.to_le_bytes());
    overlapping_tables[0x5bc..0x5c0].copy_from_slice(&0x90_i32.to_le_bytes());
    assert!(matches!(
        Document::parse(&overlapping_tables),
        Err(Error::Malformed { .. })
    ));

    let mut invalid_utf8 = SOURCE_TYPES.to_vec();
    invalid_utf8[0x608] = 0xff;
    assert!(matches!(
        Document::parse(&invalid_utf8),
        Err(Error::InvalidSourceUtf8 { node, .. }) if node.index() == 2
    ));

    assert!(matches!(
        Document::parse(&SOURCE_TYPES[..7]),
        Err(Error::InvalidHeader)
    ));
    let mut bad_identifier = SOURCE_TYPES.to_vec();
    bad_identifier[4] = b'X';
    assert!(matches!(
        Document::parse(&bad_identifier),
        Err(Error::InvalidIdentifier)
    ));

    let mut missing_pool = SOURCE_TYPES.to_vec();
    missing_pool[0x0e..0x10].copy_from_slice(&0_u16.to_le_bytes());
    assert!(matches!(
        Document::parse(&missing_pool),
        Err(Error::MissingPatchObjectPool)
    ));
}

#[test]
fn validates_replacement_handles_before_building_output() {
    let document = Document::parse(SOURCE_TYPES).unwrap();
    let handle = document.sources()[0].handle;
    assert!(matches!(
        document.rewrite(&[(handle, "first"), (handle, "second")]),
        Err(Error::DuplicateReplacement { node }) if node == handle
    ));

    let other_document = Document::parse(REPRESENTATIVE).unwrap();
    let other_handle = other_document.sources()[1].handle;
    assert_eq!(other_handle.index(), handle.index());
    assert!(matches!(
        document.rewrite(&[(other_handle, "source")]),
        Err(Error::UnknownNodeHandle { node }) if node == other_handle
    ));
}
