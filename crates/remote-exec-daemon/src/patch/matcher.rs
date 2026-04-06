#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eof: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start.min(lines.len()));
    }

    if pattern.len() > lines.len() {
        return None;
    }

    let max_start = lines.len() - pattern.len();
    let start = start.min(lines.len());
    if start > max_start {
        return None;
    }

    find_match(lines, pattern, start, max_start, eof, |left, right| left == right)
        .or_else(|| {
            find_match(lines, pattern, start, max_start, eof, |left, right| {
                left.trim_end() == right.trim_end()
            })
        })
        .or_else(|| {
            find_match(lines, pattern, start, max_start, eof, |left, right| {
                left.trim() == right.trim()
            })
        })
        .or_else(|| {
            find_match(lines, pattern, start, max_start, eof, |left, right| {
                normalize_unicode(left.trim()) == normalize_unicode(right.trim())
            })
        })
}

fn find_match<F>(
    lines: &[String],
    pattern: &[String],
    start: usize,
    max_start: usize,
    eof: bool,
    matches: F,
) -> Option<usize>
where
    F: Fn(&str, &str) -> bool,
{
    let is_match = |idx| {
        lines[idx..idx + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(left, right)| matches(left, right))
    };

    if eof {
        (start..=max_start).rev().find(|&idx| is_match(idx))
    } else {
        (start..=max_start).find(|&idx| is_match(idx))
    }
}

fn normalize_unicode(value: &str) -> String {
    value.chars().map(normalize_char).collect()
}

fn normalize_char(ch: char) -> char {
    match ch {
        '\u{00a0}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
        '\u{2018}' | '\u{2019}' | '\u{201b}' | '\u{2032}' => '\'',
        '\u{201c}' | '\u{201d}' | '\u{201f}' | '\u{2033}' => '"',
        _ => ch,
    }
}

#[cfg(test)]
mod tests {
    use super::seek_sequence;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn matches_exact_sequence() {
        let lines = strings(&["alpha", "beta", "gamma"]);
        let pattern = strings(&["beta", "gamma"]);

        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(1));
    }

    #[test]
    fn matches_ignoring_trailing_whitespace() {
        let lines = strings(&["alpha  ", "beta\t"]);
        let pattern = strings(&["alpha", "beta"]);

        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn matches_after_full_trim() {
        let lines = strings(&["  alpha  ", "\tbeta\t", "omega"]);
        let pattern = strings(&["alpha", "beta"]);

        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn matches_after_unicode_normalization() {
        let lines = strings(&["alpha — “beta\u{00a0}gamma”"]);
        let pattern = strings(&["alpha - \"beta gamma\""]);

        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn prefers_match_nearest_eof() {
        let lines = strings(&["match", "middle", "match", "tail"]);
        let pattern = strings(&["match"]);

        assert_eq!(seek_sequence(&lines, &pattern, 0, true), Some(2));
    }

    #[test]
    fn returns_none_when_pattern_is_longer_than_lines() {
        let lines = strings(&["alpha"]);
        let pattern = strings(&["alpha", "beta"]);

        assert_eq!(seek_sequence(&lines, &pattern, 0, false), None);
    }

    #[test]
    fn returns_clamped_start_for_empty_pattern() {
        let lines = strings(&["alpha", "beta"]);
        let pattern = strings(&[]);

        assert_eq!(seek_sequence(&lines, &pattern, 99, false), Some(2));
    }

    #[test]
    fn returns_none_when_start_is_past_last_possible_match() {
        let lines = strings(&["alpha", "beta", "gamma"]);
        let pattern = strings(&["beta", "gamma"]);

        assert_eq!(seek_sequence(&lines, &pattern, 2, false), None);
    }
}
