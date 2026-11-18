//! Lua 5.4 patterns, as `string.find(s, pattern)` reads them (a port of
//! `lstrlib.c`'s matcher), for `Lua.doc.regengine` `lua`: LuaLS matches
//! `Lua.doc.privateName` / `protectedName` with `string.find`.

const MAX_CAPTURES: usize = 32;
const MAX_DEPTH: usize = 200;
const UNFINISHED: isize = -1;
const POSITION: isize = -2;

/// Whether `pattern` matches somewhere in `s` (anchored with `^`). A
/// malformed pattern (where Lua raises an error) matches nothing.
pub fn find(s: &str, pattern: &str) -> bool {
    let (p, anchor) = match pattern.as_bytes() {
        [b'^', rest @ ..] => (rest, true),
        p => (p, false),
    };
    let mut m = Matcher { s: s.as_bytes(), p, level: 0, capture: [(0, 0); MAX_CAPTURES], depth: MAX_DEPTH, bad: false };
    let mut start = 0;
    loop {
        m.level = 0;
        m.depth = MAX_DEPTH;
        if m.do_match(start, 0).is_some() {
            // Lua raises "unfinished capture" when it returns the match.
            return !m.bad && m.capture[..m.level].iter().all(|c| c.1 != UNFINISHED);
        }
        if m.bad || anchor || start >= m.s.len() {
            return false;
        }
        start += 1;
    }
}

struct Matcher<'a> {
    s: &'a [u8],
    p: &'a [u8],
    level: usize,
    /// (start, length or UNFINISHED / POSITION)
    capture: [(usize, isize); MAX_CAPTURES],
    depth: usize,
    bad: bool,
}

fn class(c: u8, cl: u8) -> bool {
    let res = match cl.to_ascii_lowercase() {
        b'a' => c.is_ascii_alphabetic(),
        b'c' => c.is_ascii_control(),
        b'd' => c.is_ascii_digit(),
        b'g' => c.is_ascii_graphic(),
        b'l' => c.is_ascii_lowercase(),
        b'p' => c.is_ascii_punctuation(),
        b's' => matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c),
        b'u' => c.is_ascii_uppercase(),
        b'w' => c.is_ascii_alphanumeric(),
        b'x' => c.is_ascii_hexdigit(),
        _ => return cl == c,
    };
    if cl.is_ascii_uppercase() { !res } else { res }
}

impl Matcher<'_> {
    fn pat(&self, i: usize) -> u8 {
        self.p.get(i).copied().unwrap_or(0)
    }

    /// The end of the single-character class starting at `p`.
    fn class_end(&mut self, mut p: usize) -> usize {
        let c = self.pat(p);
        p += 1;
        if c == b'%' {
            if p >= self.p.len() {
                self.bad = true;
            }
            return p + 1;
        }
        if c == b'[' {
            if self.pat(p) == b'^' {
                p += 1;
            }
            loop {
                if p >= self.p.len() {
                    self.bad = true;
                    return self.p.len();
                }
                let cc = self.p[p];
                p += 1;
                if cc == b'%' && p < self.p.len() {
                    p += 1;
                }
                if self.pat(p) == b']' {
                    break;
                }
            }
            return p + 1;
        }
        p
    }

    /// `p` at the `[`, `ec` at its `]`.
    fn bracket(&self, c: u8, mut p: usize, ec: usize) -> bool {
        let mut sig = true;
        if self.pat(p + 1) == b'^' {
            sig = false;
            p += 1;
        }
        loop {
            p += 1;
            if p >= ec {
                break;
            }
            if self.pat(p) == b'%' {
                p += 1;
                if class(c, self.pat(p)) {
                    return sig;
                }
            } else if self.pat(p + 1) == b'-' && p + 2 < ec {
                p += 2;
                if self.pat(p - 2) <= c && c <= self.pat(p) {
                    return sig;
                }
            } else if self.pat(p) == c {
                return sig;
            }
        }
        !sig
    }

    fn single(&self, s: usize, p: usize, ep: usize) -> bool {
        let Some(&c) = self.s.get(s) else { return false };
        match self.pat(p) {
            b'.' => true,
            b'%' => class(c, self.pat(p + 1)),
            b'[' => self.bracket(c, p, ep - 1),
            pc => pc == c,
        }
    }

    fn do_match(&mut self, mut s: usize, mut p: usize) -> Option<usize> {
        if self.bad {
            return None;
        }
        if self.depth == 0 {
            self.bad = true;
            return None;
        }
        self.depth -= 1;
        let res = loop {
            if p == self.p.len() {
                break Some(s);
            }
            match self.pat(p) {
                b'(' => {
                    break if self.pat(p + 1) == b')' {
                        self.start_capture(s, p + 2, POSITION)
                    } else {
                        self.start_capture(s, p + 1, UNFINISHED)
                    };
                }
                b')' => break self.end_capture(s, p + 1),
                b'$' if p + 1 == self.p.len() => break (s == self.s.len()).then_some(s),
                b'%' if self.pat(p + 1) == b'b' => match self.balance(s, p + 2) {
                    Some(e) => {
                        s = e;
                        p += 4;
                        continue;
                    }
                    None => break None,
                },
                b'%' if self.pat(p + 1) == b'f' => {
                    p += 2;
                    if self.pat(p) != b'[' {
                        self.bad = true;
                        break None;
                    }
                    let ep = self.class_end(p);
                    if self.bad {
                        break None;
                    }
                    let prev = if s == 0 { 0 } else { self.s[s - 1] };
                    let cur = self.s.get(s).copied().unwrap_or(0);
                    if !self.bracket(prev, p, ep - 1) && self.bracket(cur, p, ep - 1) {
                        p = ep;
                        continue;
                    }
                    break None;
                }
                b'%' if self.pat(p + 1).is_ascii_digit() => match self.match_capture(s, self.pat(p + 1)) {
                    Some(e) => {
                        s = e;
                        p += 2;
                        continue;
                    }
                    None => break None,
                },
                _ => {
                    let ep = self.class_end(p);
                    if self.bad {
                        break None;
                    }
                    let next = self.pat(ep);
                    if !self.single(s, p, ep) {
                        if matches!(next, b'*' | b'?' | b'-') {
                            p = ep + 1;
                            continue;
                        }
                        break None;
                    }
                    match next {
                        b'?' => {
                            if let Some(r) = self.do_match(s + 1, ep + 1) {
                                break Some(r);
                            }
                            p = ep + 1;
                            continue;
                        }
                        b'+' => break self.max_expand(s + 1, p, ep),
                        b'*' => break self.max_expand(s, p, ep),
                        b'-' => break self.min_expand(s, p, ep),
                        _ => {
                            s += 1;
                            p = ep;
                            continue;
                        }
                    }
                }
            }
        };
        self.depth += 1;
        res
    }

    fn max_expand(&mut self, s: usize, p: usize, ep: usize) -> Option<usize> {
        let mut i = 0;
        while self.single(s + i, p, ep) {
            i += 1;
        }
        loop {
            if let Some(r) = self.do_match(s + i, ep + 1) {
                return Some(r);
            }
            if i == 0 || self.bad {
                return None;
            }
            i -= 1;
        }
    }

    fn min_expand(&mut self, mut s: usize, p: usize, ep: usize) -> Option<usize> {
        loop {
            if let Some(r) = self.do_match(s, ep + 1) {
                return Some(r);
            }
            if self.bad || !self.single(s, p, ep) {
                return None;
            }
            s += 1;
        }
    }

    fn start_capture(&mut self, s: usize, p: usize, what: isize) -> Option<usize> {
        if self.level >= MAX_CAPTURES {
            self.bad = true;
            return None;
        }
        self.capture[self.level] = (s, what);
        self.level += 1;
        let res = self.do_match(s, p);
        if res.is_none() {
            self.level -= 1;
        }
        res
    }

    fn end_capture(&mut self, s: usize, p: usize) -> Option<usize> {
        let Some(l) = (0..self.level).rev().find(|&l| self.capture[l].1 == UNFINISHED) else {
            self.bad = true;
            return None;
        };
        self.capture[l].1 = (s - self.capture[l].0) as isize;
        let res = self.do_match(s, p);
        if res.is_none() {
            self.capture[l].1 = UNFINISHED;
        }
        res
    }

    fn match_capture(&mut self, s: usize, digit: u8) -> Option<usize> {
        let l =
            (digit as usize).checked_sub(b'1' as usize).filter(|&l| l < self.level && self.capture[l].1 != UNFINISHED);
        let Some(l) = l else {
            self.bad = true;
            return None;
        };
        let (start, len) = self.capture[l];
        let len = if len == POSITION { 0 } else { len as usize };
        (self.s.len() - s >= len && self.s[start..start + len] == self.s[s..s + len]).then_some(s + len)
    }

    fn balance(&mut self, s: usize, p: usize) -> Option<usize> {
        if p + 1 >= self.p.len() {
            self.bad = true;
            return None;
        }
        let (b, e) = (self.p[p], self.p[p + 1]);
        if self.s.get(s) != Some(&b) {
            return None;
        }
        let mut cont = 1;
        let mut i = s + 1;
        while i < self.s.len() {
            let c = self.s[i];
            if c == e {
                cont -= 1;
                if cont == 0 {
                    return Some(i + 1);
                }
            } else if c == b {
                cont += 1;
            }
            i += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::find;

    #[test]
    fn lua_find() {
        // (subject, pattern, Lua 5.4's `string.find(s, p) ~= nil`)
        let cases = [
            ("m_id", "^m_", true),
            ("am_id", "^m_", false),
            ("am_id", "m_", true),
            ("_id_", "^_.*_$", true),
            ("_user", "^_.*_$", false),
            ("_user", "^_%l+$", true),
            ("_User", "^_%l+$", false),
            ("x9", "%d", true),
            ("xy", "%D%D", true),
            ("abc", "[b-c]$", true),
            ("abc", "[^abc]", false),
            ("a]b", "[%]]", true),
            ("f(a(b)c)", "%b()", true),
            ("f(a", "%b()", false),
            ("THE (quick) fox", "%f[%a]%a+", true),
            ("aa", "(a)%1", true),
            ("ab", "(a)%1", false),
            ("", "", true),
            ("", "^$", true),
            ("a", "a?b?$", true),
            ("aaab", "a-b", true),
            ("ab", "a+$", false),
            ("x", "x*", true),
            ("ab", "()b", true),
            // Malformed: an error in Lua, no match here.
            ("a", "[a", false),
            ("a", "%", false),
            ("a", "(a", false),
            ("a", "a)", false),
            ("a", "%1", false),
            ("a", "%b", false),
            ("a", "%fa", false),
        ];
        for (s, p, want) in cases {
            assert_eq!(find(s, p), want, "string.find({s:?}, {p:?})");
        }
    }
}
