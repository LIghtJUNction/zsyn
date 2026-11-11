//! Unicode's official grapheme-break conformance cases for the no_std helper.
#[allow(dead_code)]
#[clippy::msrv = "1.99.1"]
#[path = "../../src/grapheme.rs"]
mod grapheme;
#[allow(dead_code)]
#[path = "../../src/grapheme_data.rs"]
mod grapheme_data;
#[allow(dead_code)]
#[path = "../../src/unicode.rs"]
mod unicode;

#[test]
fn official_unicode_18_grapheme_break_conformance() {
    let corpus = include_str!("../corpus/GraphemeBreakTest-18.0.0.txt");
    let mut cases = 0;
    for (line_index, line) in corpus.lines().enumerate() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        let mut source = String::new();
        let mut expected = Vec::new();
        for part in line.split_whitespace() {
            match part {
                "÷" => expected.push(source.len()),
                "×" => (),
                codepoint => {
                    let value = u32::from_str_radix(codepoint, 16).unwrap();
                    source.push(char::from_u32(value).unwrap());
                }
            }
        }
        let mut actual = vec![0];
        let mut position = 0;
        while position < source.len() {
            let length = grapheme::first_len(&source[position..]);
            assert!(
                length > 0,
                "no progress in Unicode case at line {}",
                line_index + 1
            );
            position += length;
            assert!(source.is_char_boundary(position));
            actual.push(position);
        }
        assert_eq!(
            actual,
            expected,
            "Unicode conformance line {}: {source:?}",
            line_index + 1
        );
        cases += 1;
    }
    assert!(cases >= 700, "the full Unicode fixture must be present");
    assert_eq!(grapheme::first_len(""), 0);
}
