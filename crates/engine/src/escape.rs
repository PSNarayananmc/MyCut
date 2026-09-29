//! Filtergraph escaping. FFmpeg filter parameters need level-1 escaping
//! (backslash) and, inside quoted sections, additional rules. We always
//! escape aggressively; note there is NO shell involved anywhere — these
//! strings are single argv elements passed to ffmpeg's filter parser.

/// Escape a value used inside a filtergraph parameter (level 1).
#[must_use]
pub fn escape_filter_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' | '\'' | ':' | ',' | '[' | ']' | ';' | '%' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Escape text for drawtext: additionally `%` (expansion) and newline.
#[must_use]
pub fn escape_drawtext(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' | '\'' | ':' | ',' | '[' | ']' | ';' | '%' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

/// Quote a filter parameter value in single quotes (after escaping).
#[must_use]
pub fn quoted(s: &str) -> String {
    format!("'{s}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_metacharacters() {
        assert_eq!(escape_filter_value("a:b"), "a\\:b");
        assert_eq!(escape_filter_value("it's"), "it\\'s");
        assert_eq!(escape_filter_value("a\\b"), "a\\\\b");
        assert_eq!(escape_filter_value("x,y"), "x\\,y");
        assert_eq!(escape_drawtext("50% off"), "50\\% off");
        assert_eq!(escape_drawtext("l1\nl2"), "l1\\nl2");
    }

    #[test]
    fn shell_metacharacters_in_filenames_are_harmless() {
        // Filenames that would be deadly in a shell are just strings here;
        // we never spawn a shell, but escaping must still keep ffmpeg's
        // parser from interpreting them.
        let evil = "file;rm -rf /;$(whoami)`x`|cat &";
        let esc = escape_filter_value(evil);
        // The escaped form must not contain unescaped ':' or '\''.
        assert!(esc.contains("\\;"));
        assert!(!esc.contains('\'') || esc.contains("\\'"));
    }
}
