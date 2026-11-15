//! File patterns in configuration files, read as `.gitignore` reads them:
//!
//! - `*` matches within one path segment, `**` across segments, `?` one
//!   character, `{a,b}` either alternative, `[abc]` and `[!abc]` a class.
//! - A pattern without a `/` matches a file or folder name at any depth:
//!   `Templates` covers `Templates/Daily.md` and `notes/Templates/a.md`.
//! - A pattern with a `/` is relative to the folder of its configuration
//!   file; a leading `/` only makes that explicit.
//! - A pattern matching a folder covers everything in it.

use regex::Regex;

#[derive(Clone, Debug)]
pub(crate) struct Glob {
    regex: Regex,
}

impl Glob {
    pub fn new(pattern: &str) -> Result<Self, String> {
        let trimmed = pattern.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return Err(format!("empty file pattern `{pattern}`"));
        }
        let (anchored, body) = match trimmed.strip_prefix('/') {
            Some(body) => (true, body),
            None => (trimmed.contains('/'), trimmed),
        };
        let mut regex = String::from(if anchored { "^" } else { "^(?:.*/)?" });
        translate(body, &mut regex)
            .map_err(|error| format!("file pattern `{pattern}`: {error}"))?;
        regex.push_str("(?:/.*)?$");
        let regex =
            Regex::new(&regex).map_err(|error| format!("file pattern `{pattern}`: {error}"))?;
        Ok(Self { regex })
    }

    /// Whether `path`, relative to the configuration's folder and with `/`
    /// between segments, matches.
    pub fn matches(&self, path: &str) -> bool {
        self.regex.is_match(path)
    }
}

fn translate(pattern: &str, out: &mut String) -> Result<(), String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut ix = 0;
    let mut in_alternatives = false;
    while ix < chars.len() {
        let ch = chars[ix];
        match ch {
            '*' if chars.get(ix + 1) == Some(&'*') => {
                let at_segment_start = ix == 0 || chars[ix - 1] == '/';
                if at_segment_start && chars.get(ix + 2) == Some(&'/') {
                    // `**/` matches no folder or any number of them.
                    out.push_str("(?:.*/)?");
                    ix += 3;
                    continue;
                }
                out.push_str(".*");
                ix += 2;
                continue;
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            '{' if !in_alternatives => {
                in_alternatives = true;
                out.push_str("(?:");
            }
            ',' if in_alternatives => out.push('|'),
            '}' if in_alternatives => {
                in_alternatives = false;
                out.push(')');
            }
            '[' => {
                let close = chars[ix + 1..]
                    .iter()
                    .position(|ch| *ch == ']')
                    .ok_or("unclosed `[`")?
                    + ix
                    + 1;
                out.push('[');
                let mut class = &chars[ix + 1..close];
                if let Some(('!', rest)) = class.split_first() {
                    out.push('^');
                    class = rest;
                }
                for ch in class {
                    if matches!(ch, '\\' | '[' | ']' | '^') {
                        out.push('\\');
                    }
                    out.push(*ch);
                }
                out.push(']');
                ix = close + 1;
                continue;
            }
            _ => out.push_str(&regex::escape(ch.encode_utf8(&mut [0; 4]))),
        }
        ix += 1;
    }
    if in_alternatives {
        return Err("unclosed `{`".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, path: &str) -> bool {
        Glob::new(pattern).unwrap().matches(path)
    }

    #[test]
    fn names_match_at_any_depth() {
        assert!(matches("Templates", "Templates/Daily.md"));
        assert!(matches("Templates", "notes/Templates/a.md"));
        assert!(matches("*.excalidraw.md", "a/b/x.excalidraw.md"));
        assert!(!matches("Templates", "TemplatesOld/a.md"));
    }

    #[test]
    fn paths_are_relative_to_the_configuration() {
        assert!(matches("Journal/**", "Journal/2026/a.md"));
        assert!(!matches("Journal/**", "notes/Journal/a.md"));
        assert!(matches("/README.md", "README.md"));
        assert!(!matches("/README.md", "docs/README.md"));
        assert!(matches("docs/**/*.md", "docs/a.md"));
        assert!(matches("docs/**/*.md", "docs/x/y/a.md"));
    }

    #[test]
    fn classes_and_alternatives() {
        assert!(matches("{a,b}.md", "b.md"));
        assert!(!matches("{a,b}.md", "c.md"));
        assert!(matches("[!x]*.md", "note.md"));
        assert!(!matches("[!x]*.md", "xnote.md"));
        assert!(matches("note?.md", "note1.md"));
        assert!(Glob::new("[abc").is_err());
        assert!(Glob::new("{a,b").is_err());
        assert!(Glob::new(" ").is_err());
    }
}
