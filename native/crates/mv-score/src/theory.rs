//! Scales, chords and progressions, plus the small seeded random generator the composer uses (the same
//! chapter and settings always give the same score).

const NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

pub fn note_name(pc: u8) -> &'static str {
    NAMES[(pc % 12) as usize]
}

/// Semitone steps of a mode, from the tonic.
pub fn scale(mode: &str) -> &'static [u8] {
    match mode {
        "major" => &[0, 2, 4, 5, 7, 9, 11],
        "lydian" => &[0, 2, 4, 6, 7, 9, 11],
        "dorian" => &[0, 2, 3, 5, 7, 9, 10],
        "phrygian" => &[0, 1, 3, 5, 7, 8, 10],
        "harmonic minor" => &[0, 2, 3, 5, 7, 8, 11],
        _ => &[0, 2, 3, 5, 7, 8, 10], // natural minor
    }
}

/// MIDI note of scale degree `deg` (0-based, may exceed 7 or be negative) above `root_midi`.
pub fn degree(root_midi: i32, mode: &str, deg: i32) -> i32 {
    let s = scale(mode);
    let n = s.len() as i32;
    let oct = deg.div_euclid(n);
    root_midi + oct * 12 + s[deg.rem_euclid(n) as usize] as i32
}

/// A diatonic chord on degree `deg`: root, third, fifth (and seventh when `seventh`).
pub fn chord(root_midi: i32, mode: &str, deg: i32, seventh: bool) -> Vec<i32> {
    let mut v = vec![degree(root_midi, mode, deg), degree(root_midi, mode, deg + 2), degree(root_midi, mode, deg + 4)];
    if seventh {
        v.push(degree(root_midi, mode, deg + 6));
    }
    v
}

/// Chord progressions (scale degrees, 0 = tonic) that suit a style, one chord per bar.
pub fn progressions(style: crate::Style) -> &'static [&'static [i32]] {
    use crate::Style::*;
    match style {
        // i - VI - III - VII: the anime battle staple; i - iv - VI - V for drive.
        Action | Epic => &[&[0, 5, 2, 6], &[0, 3, 5, 4], &[0, 5, 6, 4], &[5, 6, 0, 0]],
        Tense | Mystery => &[&[0, 0, 1, 0], &[0, 5, 0, 1], &[0, 3, 0, 5]],
        Emotional => &[&[0, 5, 2, 6], &[5, 3, 0, 4], &[0, 3, 5, 4], &[3, 4, 2, 5]],
        Romance => &[&[0, 4, 5, 3], &[3, 4, 2, 5], &[0, 5, 3, 4]],
        Comedy => &[&[0, 3, 4, 0], &[0, 5, 3, 4], &[0, 1, 4, 0]],
        Calm => &[&[0, 3, 0, 4], &[0, 5, 3, 4], &[3, 0, 4, 0]],
        Flashback => &[&[0, 5, 3, 4], &[0, 2, 5, 4]],
        Reveal => &[&[0, 1, 0, 5], &[0, 5, 1, 0]],
    }
}

/// xorshift64*: tiny, fast and stable across platforms and versions.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// 0..1
    pub fn f(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.f() < p
    }

    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.range(xs.len())]
    }
}

/// FNV-1a, for seeds from strings.
pub fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degrees_wrap_octaves() {
        assert_eq!(degree(60, "major", 0), 60);
        assert_eq!(degree(60, "major", 7), 72);
        assert_eq!(degree(60, "minor", -1), 58);
        assert_eq!(chord(57, "minor", 0, false), vec![57, 60, 64]);
    }

    #[test]
    fn rng_is_deterministic() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..10 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        assert!((0.0..1.0).contains(&Rng::new(3).f()));
    }
}
