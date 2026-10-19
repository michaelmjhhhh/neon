/// Counts words across text pushed in pieces, so a rope can be counted chunk
/// by chunk without flattening it.
///
/// The rules follow common word processors:
/// - Scripts written with spaces count one word per run of non-space text.
///   A run needs a letter or digit, so Markdown markers such as `#` or `-`
///   do not count.
/// - Chinese and Japanese are written without spaces, so each Han, kana, or
///   Bopomofo character counts as one word. Korean uses spaces and is
///   counted like Latin text.
/// - East Asian punctuation separates words, as a space would.
#[derive(Clone, Copy, Debug, Default)]
pub struct WordCounter {
    words: usize,
    in_run: bool,
    run_counted: bool,
}

impl WordCounter {
    pub fn push(&mut self, text: &str) {
        for character in text.chars() {
            if is_counted_per_character(character) {
                self.words += 1;
                self.in_run = false;
            } else if character.is_whitespace() || is_east_asian_punctuation(character) {
                self.in_run = false;
            } else {
                if !self.in_run {
                    self.in_run = true;
                    self.run_counted = false;
                }
                if !self.run_counted && character.is_alphanumeric() {
                    self.words += 1;
                    self.run_counted = true;
                }
            }
        }
    }

    pub fn words(&self) -> usize {
        self.words
    }
}

pub fn count_words(text: &str) -> usize {
    let mut counter = WordCounter::default();
    counter.push(text);
    counter.words()
}

fn is_counted_per_character(character: char) -> bool {
    matches!(
        character,
        // Ideographic iteration mark, closing mark, and number zero.
        '\u{3005}'..='\u{3007}'
            // Hiragana, Katakana, Bopomofo.
            | '\u{3040}'..='\u{30FF}'
            | '\u{3100}'..='\u{312F}'
            | '\u{31A0}'..='\u{31BF}'
            | '\u{31F0}'..='\u{31FF}'
            // CJK Unified Ideographs, Extension A, and compatibility forms.
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            // Halfwidth Katakana.
            | '\u{FF66}'..='\u{FF9F}'
            // Extensions B through I and compatibility supplement.
            | '\u{20000}'..='\u{323AF}'
    )
}

fn is_east_asian_punctuation(character: char) -> bool {
    match character {
        // CJK Symbols and Punctuation, CJK Compatibility Forms.
        '\u{3000}'..='\u{303F}' | '\u{FE30}'..='\u{FE4F}' => true,
        // Fullwidth forms: punctuation separates, fullwidth letters and digits
        // stay part of a word.
        '\u{FF00}'..='\u{FFEF}' => !character.is_alphanumeric(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_space_separated_words() {
        assert_eq!(count_words("one two\nthree"), 3);
        assert_eq!(count_words("  it's an e-mail, 3.14 "), 4);
        assert_eq!(count_words("Привет мир"), 2);
        assert_eq!(count_words("안녕하세요 세계"), 2);
    }

    #[test]
    fn skips_runs_without_letters_or_digits() {
        assert_eq!(count_words("# Title\n\n- item --- **"), 2);
        assert_eq!(count_words(""), 0);
    }

    #[test]
    fn counts_each_chinese_and_japanese_character() {
        assert_eq!(count_words("权力结构"), 4);
        assert_eq!(count_words("鲁迅：《孔乙己》《药》"), 6);
        assert_eq!(
            count_words("福克纳:《献给艾米丽的一朵玫瑰花》《烧马棚》"),
            17
        );
        assert_eq!(count_words("日本語のテキスト"), 8);
        assert_eq!(count_words("学习Rust语言"), 5);
    }

    #[test]
    fn east_asian_punctuation_separates_words() {
        assert_eq!(count_words("hello，world。"), 2);
        assert_eq!(count_words("ＡＢＣ　ＤＥＦ"), 2);
    }

    #[test]
    fn counts_across_pushed_pieces() {
        let mut counter = WordCounter::default();
        for piece in ["hel", "lo wo", "rld 中", "文"] {
            counter.push(piece);
        }
        assert_eq!(counter.words(), 4);
    }
}
