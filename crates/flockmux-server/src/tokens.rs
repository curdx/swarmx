//! Cheap, dependency-free token *estimate* (CJK-aware).
//!
//! flockmux can't get exact token counts without the model's tokenizer, and the
//! PTY-driven CLIs manage their OWN context window — so this is only used for
//! observability + deciding whether a blackboard ledger is worth compacting.
//! Heuristic: ~1 token per CJK character, ~1 token per 4 other characters
//! (the common English ≈ 4 chars/token rule). Good enough to show "this ledger
//! is ~12k tokens" and to report before/after a compaction.

/// Estimated token count for `text`.
pub fn estimate(text: &str) -> usize {
    let mut other = 0usize;
    let mut cjk = 0usize;
    for c in text.chars() {
        if is_cjk(c as u32) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + other.div_ceil(4)
}

fn is_cjk(u: u32) -> bool {
    (0x3000..=0x9FFF).contains(&u)      // CJK symbols, Hiragana/Katakana, CJK Unified
        || (0xAC00..=0xD7AF).contains(&u) // Hangul syllables
        || (0xF900..=0xFAFF).contains(&u) // CJK compatibility ideographs
        || (0x20000..=0x2FA1F).contains(&u) // CJK Extension B+
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(estimate(""), 0);
    }

    #[test]
    fn ascii_about_four_chars_per_token() {
        // 12 chars → ceil(12/4) = 3
        assert_eq!(estimate("hello world!"), 3);
    }

    #[test]
    fn cjk_about_one_per_char() {
        assert_eq!(estimate("你好世界"), 4); // 4 CJK chars
    }

    #[test]
    fn mixed() {
        // "任务: done" → 2 CJK (任务) + 7 others ("：" is CJK punct in 0x3000 range
        // → counts as CJK; ": done" remainder). Just assert it's in a sane band.
        let n = estimate("任务完成 done");
        assert!(n >= 4 && n <= 12, "got {n}");
    }
}
