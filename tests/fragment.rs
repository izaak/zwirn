use std::borrow::Cow;

use zwirn::adls::{Document, NodeKind};
use zwirn::fragment::{
    BaselineHash, BaselineHashError, CanonicalSource, CanonicalSourceError, FragmentPath,
    FragmentPathError, FragmentUpdate, ParseError, ParsedSource, RewriteError,
};

const REPRESENTATIVE: &[u8] = include_bytes!("fixtures/representative.audulus4");

#[test]
fn fragment_paths_accept_only_canonical_identity_strings() {
    for valid in ["source.lua", "src/filter/svf.lua", "λ/雪.lyte", "a:b"] {
        let path = FragmentPath::try_from(valid).unwrap();
        assert_eq!(path.as_str(), valid);
        assert_eq!(path.to_string(), valid);
    }

    let invalid = [
        ("", FragmentPathError::Empty),
        ("/source.lua", FragmentPathError::LeadingSlash),
        ("source/", FragmentPathError::TrailingSlash),
        ("source\\file.lua", FragmentPathError::Backslash),
        ("source file.lua", FragmentPathError::Whitespace),
        ("source\u{2003}file.lua", FragmentPathError::Whitespace),
        ("source//file.lua", FragmentPathError::EmptySegment),
        (
            "source/./file.lua",
            FragmentPathError::CurrentDirectorySegment,
        ),
        (
            "source/../file.lua",
            FragmentPathError::ParentDirectorySegment,
        ),
    ];
    for (source, expected) in invalid {
        assert_eq!(FragmentPath::try_from(source), Err(expected), "{source:?}");
    }
}

#[test]
fn canonical_source_normalizes_only_bom_and_line_representation() {
    let cases = [
        ("", ""),
        ("one", "one\n"),
        ("one\n", "one\n"),
        ("one\r\ntwo\rthree", "one\ntwo\nthree\n"),
        ("\r\r\n", "\n\n"),
        ("雪  \t\n\n", "雪  \t\n\n"),
        ("a\0b", "a\0b\n"),
        ("a\u{feff}b", "a\u{feff}b\n"),
    ];
    for (source, expected) in cases {
        let canonical = CanonicalSource::try_from(source).unwrap();
        assert_eq!(canonical.as_str(), expected, "{source:?}");
        assert_eq!(
            CanonicalSource::try_from(canonical.as_str()).unwrap(),
            canonical,
            "canonicalization should be idempotent"
        );
    }

    assert_eq!(
        CanonicalSource::try_from("\u{feff}source"),
        Err(CanonicalSourceError::ByteOrderMark)
    );
    assert_eq!(
        CanonicalSource::try_from(String::from("owned"))
            .unwrap()
            .as_str(),
        "owned\n"
    );
}

#[test]
fn baseline_hash_parses_formats_and_hashes_canonical_bytes() {
    let expected: BaselineHash = "e3b0c44298fc1c14".parse().unwrap();
    let empty = CanonicalSource::try_from("").unwrap();
    assert_eq!(BaselineHash::from_source(&empty), expected);
    assert_eq!(expected.to_string(), "e3b0c44298fc1c14");
    assert_eq!(
        expected.as_bytes(),
        &[0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14]
    );

    assert_eq!(
        BaselineHash::try_from("e3b0"),
        Err(BaselineHashError::InvalidLength { actual: 4 })
    );
    assert_eq!(
        BaselineHash::try_from("E3b0c44298fc1c14"),
        Err(BaselineHashError::InvalidCharacter { index: 0 })
    );
    assert_eq!(
        BaselineHash::try_from("g3b0c44298fc1c14"),
        Err(BaselineHashError::InvalidCharacter { index: 0 })
    );
}

#[test]
fn parses_each_node_comment_syntax_and_exact_physical_lines() {
    for (kind, comment) in [
        (NodeKind::Canvas, "--"),
        (NodeKind::Dsp, "--"),
        (NodeKind::Shader, "//"),
        (NodeKind::LyteDsp, "//"),
    ] {
        let source = format!(
            "wrong comment // @{{ ignored\n\t{comment}\t@{{\t src/雪.code  \r\nfirst\rsecond\n  {comment}  @}}  src/雪.code\t0123456789abcdef\t"
        );
        let parsed = ParsedSource::parse(kind, &source).unwrap();
        let fragments = parsed.fragments().collect::<Vec<_>>();
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].path.as_str(), "src/雪.code");
        assert_eq!(fragments[0].source, "first\rsecond\n");
        assert_eq!(
            fragments[0].baseline.unwrap().to_string(),
            "0123456789abcdef"
        );
    }

    let lookalikes = "--@{ joined\n-- @{joined\n// @{ wrong\n-- ordinary @} text\n";
    assert!(
        ParsedSource::parse(NodeKind::Dsp, lookalikes)
            .unwrap()
            .fragments()
            .next()
            .is_none()
    );

    let empty = ParsedSource::parse(NodeKind::Dsp, "-- @{ empty.lua\r-- @} empty.lua").unwrap();
    assert_eq!(empty.fragments().next().unwrap().source, "");
}

#[test]
fn reports_the_first_structural_marker_error_with_line_context() {
    let cases = [
        ("-- @} path.lua", ParseError::OrphanedClosing { line: 1 }),
        (
            "-- @{ first.lua\n-- @{ second.lua\n-- @} second.lua",
            ParseError::NestedOpening {
                line: 2,
                opening_line: 1,
            },
        ),
        (
            "-- @{ first.lua\n-- @} second.lua",
            ParseError::MismatchedClosingPath {
                line: 2,
                opening_line: 1,
                expected: FragmentPath::try_from("first.lua").unwrap(),
                found: FragmentPath::try_from("second.lua").unwrap(),
            },
        ),
        (
            "line one\n-- @{ path.lua",
            ParseError::Unterminated {
                line: 2,
                path: FragmentPath::try_from("path.lua").unwrap(),
            },
        ),
        ("-- @{", ParseError::MissingPath { line: 1 }),
        (
            "-- @{ ../path.lua",
            ParseError::InvalidPath {
                line: 1,
                source: FragmentPathError::ParentDirectorySegment,
            },
        ),
        (
            "-- @{ path.lua hash",
            ParseError::UnexpectedTokens { line: 1 },
        ),
        (
            "-- @{ path.lua\n-- @} path.lua ABCDEF0123456789",
            ParseError::InvalidHash {
                line: 2,
                source: BaselineHashError::InvalidCharacter { index: 0 },
            },
        ),
    ];

    for (source, expected) in cases {
        assert_eq!(
            ParsedSource::parse(NodeKind::Dsp, source).unwrap_err(),
            expected
        );
    }

    let bom = "-- @{ path.lua\n\u{feff}source\n-- @} path.lua";
    assert!(matches!(
        ParsedSource::parse(NodeKind::Dsp, bom),
        Err(ParseError::InvalidFragmentSource {
            line: 2,
            source: CanonicalSourceError::ByteOrderMark,
            ..
        })
    ));
}

#[test]
fn rewrites_multiple_fragments_surgically_in_source_order() {
    let source = concat!(
        "prefix\r\n",
        "  --  @{  first.lua\t\r\n",
        "old first\r\n",
        "  --\t@}\tfirst.lua\t  \r\n",
        "middle\n",
        "-- @{ second.lua\n",
        "old second\n",
        "-- @} second.lua  0000000000000000\t\n",
        "between\n",
        "-- @{ third.lua\n",
        "long old third\n",
        "-- @} third.lua  1111111111111111\n",
        "-- @{ fourth.lua\n",
        "not empty\n",
        "-- @} fourth.lua\n",
        "suffix"
    );
    let parsed = ParsedSource::parse(NodeKind::Dsp, source).unwrap();
    let fragments = parsed.fragments().collect::<Vec<_>>();
    let first = fragments[0].path;
    let second = fragments[1].path;
    let third = fragments[2].path;
    let fourth = fragments[3].path;
    let longer = CanonicalSource::try_from("new second\rline").unwrap();
    let shorter_multibyte = CanonicalSource::try_from("雪").unwrap();
    let empty = CanonicalSource::try_from("").unwrap();

    let rewritten = parsed
        .rewrite(&[
            FragmentUpdate::Replace {
                path: fourth,
                source: &empty,
            },
            FragmentUpdate::Replace {
                path: second,
                source: &longer,
            },
            FragmentUpdate::Record { path: first },
            FragmentUpdate::Replace {
                path: third,
                source: &shorter_multibyte,
            },
        ])
        .unwrap();
    let Cow::Owned(rewritten) = rewritten else {
        panic!("effective updates should produce owned source");
    };
    let first_hash =
        BaselineHash::from_source(&CanonicalSource::try_from("old first\r\n").unwrap());
    let second_hash = BaselineHash::from_source(&longer);
    let third_hash = BaselineHash::from_source(&shorter_multibyte);
    let fourth_hash = BaselineHash::from_source(&empty);
    let expected = format!(
        concat!(
            "prefix\r\n",
            "  --  @{{  first.lua\t\r\n",
            "old first\r\n",
            "  --\t@}}\tfirst.lua {first_hash}\t  \r\n",
            "middle\n",
            "-- @{{ second.lua\n",
            "new second\nline\n",
            "-- @}} second.lua  {second_hash}\t\n",
            "between\n",
            "-- @{{ third.lua\n",
            "雪\n",
            "-- @}} third.lua  {third_hash}\n",
            "-- @{{ fourth.lua\n",
            "-- @}} fourth.lua {fourth_hash}\n",
            "suffix"
        ),
        first_hash = first_hash,
        second_hash = second_hash,
        third_hash = third_hash,
        fourth_hash = fourth_hash
    );
    assert_eq!(rewritten, expected);

    let reparsed = ParsedSource::parse(NodeKind::Dsp, &rewritten).unwrap();
    let no_change = reparsed
        .rewrite(&[
            FragmentUpdate::Record {
                path: reparsed.fragments().nth(1).unwrap().path,
            },
            FragmentUpdate::Record {
                path: reparsed.fragments().next().unwrap().path,
            },
        ])
        .unwrap();
    assert!(matches!(no_change, Cow::Borrowed(_)));
}

#[test]
fn rewrite_rejects_duplicate_unknown_and_ambiguous_paths() {
    let source = "-- @{ same.lua\none\n-- @} same.lua\n-- @{ same.lua\ntwo\n-- @} same.lua";
    let parsed = ParsedSource::parse(NodeKind::Dsp, source).unwrap();
    let same = FragmentPath::try_from("same.lua").unwrap();
    let unknown = FragmentPath::try_from("unknown.lua").unwrap();

    assert_eq!(
        parsed.rewrite(&[
            FragmentUpdate::Record { path: &same },
            FragmentUpdate::Record { path: &same },
        ]),
        Err(RewriteError::DuplicateUpdate { path: same.clone() })
    );
    assert_eq!(
        parsed.rewrite(&[FragmentUpdate::Record { path: &unknown }]),
        Err(RewriteError::UnknownPath {
            path: unknown.clone(),
        })
    );
    assert_eq!(
        parsed.rewrite(&[FragmentUpdate::Record { path: &same }]),
        Err(RewriteError::AmbiguousPath { path: same })
    );
}

#[test]
fn representative_document_composes_adls_and_fragment_discovery() {
    let document = Document::parse(REPRESENTATIVE).unwrap();
    let mut discovered = Vec::new();

    for node in document.sources() {
        let parsed = ParsedSource::parse(node.kind, node.source).unwrap();
        for fragment in parsed.fragments() {
            discovered.push((
                fragment.path.as_str().to_owned(),
                node.kind,
                fragment.baseline,
                fragment.source.to_owned(),
            ));
        }
    }
    discovered.sort_by(|left, right| left.0.cmp(&right.0));

    assert_eq!(
        discovered,
        [
            (
                "angular_smoother.lua".to_owned(),
                NodeKind::Dsp,
                None,
                include_str!("fixtures/angular_smoother.lua").to_owned(),
            ),
            (
                "angular_smoother.lyte".to_owned(),
                NodeKind::LyteDsp,
                None,
                include_str!("fixtures/angular_smoother.lyte").to_owned(),
            ),
        ]
    );
}
