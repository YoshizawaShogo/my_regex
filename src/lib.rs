mod error;
mod nfa;
mod parse;
mod token;

use crate::nfa::build_nfa;
use crate::parse::{insert_concat, to_postfix};
use crate::token::tokenize;
use crate::{
    error::Error,
    nfa::{Label, State},
};

#[derive(Clone, Debug)]
pub struct Regex {
    states: Vec<State>,
    start: usize,
    accept: usize,
    anchored_start: bool,
    anchored_end: bool,
}

impl Regex {
    pub fn new(pat: &str) -> Result<Self, Error> {
        // 簡易アンカー処理（先頭 '^' と末尾 '$' を見るだけ。エスケープは未対応）
        let mut anchored_start = false;
        let mut anchored_end = false;
        let mut body = pat;

        if let Some(rest) = body.strip_prefix('^') {
            anchored_start = true;
            body = rest;
        }
        if let Some(rest) = body.strip_suffix('$') {
            anchored_end = true;
            body = rest;
        }

        // 通常のパイプライン
        let tokens = tokenize(body)?;
        let tokens = insert_concat(&tokens);
        let postfix = to_postfix(&tokens)?;
        let nfa = build_nfa(&postfix)?;

        Ok(Self {
            states: nfa.states,
            start: nfa.start,
            accept: nfa.accept,
            anchored_start,
            anchored_end,
        })
    }

    pub fn is_match(&self, hay: &str) -> bool {
        self.find(hay).is_some()
    }

    pub fn find(&self, hay: &str) -> Option<(usize, usize)> {
        let bytes = hay.as_bytes();
        let n = bytes.len();
        let start_pos_iter: Box<dyn Iterator<Item = usize>> = if self.anchored_start {
            Box::new(std::iter::once(0))
        } else {
            Box::new(0..=n)
        };
        for start_pos in start_pos_iter {
            if let Some(end) = self.match_from(bytes, start_pos) {
                if !self.anchored_end || end == n {
                    return Some((start_pos, end));
                }
            }
        }
        None
    }

    pub fn find_iter<'a>(&'a self, hay: &'a str) -> FindIter<'a> {
        FindIter {
            re: self,
            hay,
            pos: 0,
        }
    }

    fn match_from(&self, bytes: &[u8], mut i: usize) -> Option<usize> {
        let n = bytes.len();
        let mut curr = vec![self.start];
        eps_closure(&self.states, &mut curr);

        let mut last_accept: Option<usize> = None;

        while i <= n {
            // 受理到達チェック
            if curr.iter().any(|&s| s == self.accept) {
                last_accept = Some(i);
                if self.anchored_end {
                    break; // 末尾アンカーがあるならここで終了
                }
            }
            if i == n {
                break;
            }

            let b = bytes[i];
            let mut next = Vec::new();

            for &s in &curr {
                for (lbl, tgt) in &self.states[s].edges {
                    match lbl {
                        Label::Byte(c) => {
                            if *c == b {
                                next.push(*tgt);
                            }
                        }
                        Label::Any => {
                            next.push(*tgt);
                        }
                        Label::Class { ranges, neg } => {
                            let mut hit = false;
                            for &(lo, hi) in ranges {
                                if lo <= b && b <= hi {
                                    hit = true;
                                    break;
                                }
                            }
                            if (*neg && !hit) || (!*neg && hit) {
                                next.push(*tgt);
                            }
                        }
                        Label::Eps => { /* ε は eps_closure で処理する */ }
                    }
                }
            }

            if next.is_empty() {
                break;
            }

            next.sort_unstable();
            next.dedup();
            eps_closure(&self.states, &mut next);
            curr = next;
            i += 1;
        }

        last_accept
    }
}

pub struct FindIter<'a> {
    re: &'a Regex,
    hay: &'a str,
    pos: usize,
}

impl Iterator for FindIter<'_> {
    type Item = (usize, usize);
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos > self.hay.len() {
            return None;
        }
        let bytes = self.hay.as_bytes();
        let n = bytes.len();
        let mut start_pos = if self.re.anchored_start { 0 } else { self.pos };

        while start_pos <= n {
            if let Some(end) = self.re.match_from(bytes, start_pos) {
                if !self.re.anchored_end || end == n {
                    // ゼロ長対策
                    self.pos = if end > start_pos { end } else { start_pos + 1 };
                    return Some((start_pos, end));
                }
            }
            if self.re.anchored_start {
                break;
            }
            start_pos += 1;
        }
        self.pos = n + 1;
        None
    }
}

// ---- ε-閉包 ----
// set に含まれる各状態から ε 遷移で到達できる状態をすべて追加する
fn eps_closure(states: &[State], set: &mut Vec<usize>) {
    let mut stack = set.clone();
    let mut seen = vec![false; states.len()];
    for &s in set.iter() {
        if s < seen.len() {
            seen[s] = true;
        }
    }

    while let Some(s) = stack.pop() {
        if s >= states.len() {
            continue;
        }
        for (lbl, tgt) in &states[s].edges {
            if matches!(lbl, Label::Eps) {
                let t = *tgt;
                if t < seen.len() && !seen[t] {
                    seen[t] = true;
                    set.push(t);
                    stack.push(t);
                }
            }
        }
    }

    // set の順序は問わないが、必要なら正規化
    set.sort_unstable();
    set.dedup();
}

// ===== minimal tests (run with `cargo test`)
#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, s: &str) -> bool {
        Regex::new(p).unwrap().is_match(s)
    }

    #[test]
    fn lit() {
        assert!(m("abc", "xxabcyy"));
    }
    #[test]
    fn dot() {
        assert!(m("a.c", "abc"));
        assert!(m("a.c", "a c"));
    }
    #[test]
    fn star() {
        assert!(m("ab*c", "ac"));
        assert!(m("ab*c", "abbbc"));
    }
    #[test]
    fn plus() {
        assert!(m("ab+c", "abc"));
        assert!(!m("ab+c", "ac"));
    }
    #[test]
    fn qmark() {
        assert!(m("ab?c", "ac"));
        assert!(m("ab?c", "abc"));
    }
    #[test]
    fn alt() {
        assert!(m("(ab|cd)ef", "abef"));
        assert!(m("(ab|cd)ef", "cdef"));
    }
    #[test]
    fn class() {
        assert!(m("[a-cx]", "x"));
        assert!(m("[a-cx]", "b"));
        assert!(!m("[a-c]", "z"));
    }
    #[test]
    fn negclass() {
        assert!(m("[^0-9]", "a"));
        assert!(!m("[^0-9]", "5"));
    }
    #[test]
    fn anchor_start() {
        assert_eq!(Regex::new("^abc").unwrap().find("abcxx").unwrap(), (0, 3));
        assert!(Regex::new("^a").unwrap().find("ba").is_none());
    }
    #[test]
    fn anchor_end() {
        assert_eq!(Regex::new("abc$").unwrap().find("xxabc").unwrap(), (2, 5));
        assert!(Regex::new("a$").unwrap().find("ab").is_none());
    }
    #[test]
    fn find_iter() {
        let re = Regex::new("a+").unwrap();
        let s = "caaabaaa";
        let v: Vec<_> = re.find_iter(s).collect();
        assert_eq!(v, vec![(1, 4), (5, 8)]);
    }
}
