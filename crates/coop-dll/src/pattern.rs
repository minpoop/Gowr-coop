//! Byte signatures ("AOBs"): find code in the game by what it looks like instead of
//! where it is, so offsets survive game patches.
//!
//! A pattern is hex bytes with `??` wildcards: `"48 8B 05 ?? ?? ?? ?? 48 85 C0"`.

#[derive(Debug, Clone, PartialEq)]
pub struct Pattern(Vec<Option<u8>>);

impl Pattern {
    pub fn parse(text: &str) -> Result<Self, String> {
        let bytes = text
            .split_whitespace()
            .map(|t| match t {
                "?" | "??" => Ok(None),
                hex => u8::from_str_radix(hex, 16).map(Some).map_err(|_| format!("bad byte `{hex}` in pattern")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if bytes.is_empty() || bytes.iter().all(Option::is_none) {
            return Err("pattern needs at least one fixed byte".into());
        }
        Ok(Pattern(bytes))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn matches_at(&self, hay: &[u8], i: usize) -> bool {
        self.0.iter().enumerate().all(|(j, b)| b.map_or(true, |b| hay[i + j] == b))
    }

    /// Every offset in `hay` where the pattern matches.
    pub fn find_all(&self, hay: &[u8]) -> Vec<usize> {
        if hay.len() < self.0.len() {
            return Vec::new();
        }
        (0..=hay.len() - self.0.len()).filter(|&i| self.matches_at(hay, i)).collect()
    }
}

/// Resolve a RIP-relative operand: for an instruction at `insn` whose 32-bit displacement
/// sits `disp_at` bytes in and which is `insn_len` long, return the address it points to.
/// Example: `48 8B 05 <disp32>` (mov rax, [rip+disp]) has disp_at = 3, insn_len = 7.
pub fn rip_relative(code: &[u8], insn: usize, disp_at: usize, insn_len: usize) -> Option<usize> {
    let d = code.get(insn + disp_at..insn + disp_at + 4)?;
    let disp = i32::from_le_bytes(d.try_into().ok()?) as i64;
    let target = insn as i64 + insn_len as i64 + disp;
    usize::try_from(target).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_with_wildcards() {
        let code = [0x90, 0x48, 0x8B, 0x05, 0x10, 0x00, 0x00, 0x00, 0x48, 0x85, 0xC0, 0x90];
        let p = Pattern::parse("48 8B 05 ?? ?? ?? ?? 48 85 C0").unwrap();
        assert_eq!(p.find_all(&code), vec![1]);
        assert_eq!(rip_relative(&code, 1, 3, 7), Some(1 + 7 + 0x10));
    }

    #[test]
    fn bad_patterns_are_rejected() {
        assert!(Pattern::parse("").is_err());
        assert!(Pattern::parse("?? ??").is_err());
        assert!(Pattern::parse("4G").is_err());
    }

    #[test]
    fn short_haystack() {
        assert!(Pattern::parse("01 02 03").unwrap().find_all(&[1, 2]).is_empty());
    }
}
