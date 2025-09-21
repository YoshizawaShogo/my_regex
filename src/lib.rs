// lib.rs
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
}

impl Regex {
    pub fn new(pat: &str) -> Result<Self, Error> {
        // アンカーは常に有効（^…$ を暗黙）
        let body = pat;

        let tokens = tokenize(body)?;
        let tokens = insert_concat(&tokens);
        let postfix = to_postfix(&tokens)?;
        let nfa = build_nfa(&postfix)?;

        Ok(Self {
            states: nfa.states,
            start: nfa.start,
            accept: nfa.accept,
        })
    }

    pub fn is_match(&self, hay: &str) -> bool {
        let bytes = hay.as_bytes();
        match self.match_from(bytes, 0) {
            Some(end) => end == bytes.len(), // 全消費のみOK
            None => false,
        }
    }

    fn match_from(&self, bytes: &[u8], mut i: usize) -> Option<usize> {
        let n = bytes.len();
        let mut curr = vec![self.start];
        eps_closure(&self.states, &mut curr);

        let mut last_accept: Option<usize> = None;

        while i <= n {
            if curr.iter().any(|&s| s == self.accept) {
                last_accept = Some(i);
                // anchored_end は常に有効扱いするので break せず「最後まで読めるだけ読む」か、
                // ここで `break` してもOK。どちらでも `find/is_match` 側で end==n を要求するため結果は同じ。
            }
            if i == n { break; }

            let b = bytes[i];
            let mut next = Vec::new();

            for &s in &curr {
                for (lbl, tgt) in &self.states[s].edges {
                    match lbl {
                        Label::Byte(c) => if *c == b { next.push(*tgt); }
                        Label::Any => { next.push(*tgt); }
                        Label::Class { ranges, neg } => {
                            let mut hit = false;
                            for &(lo, hi) in ranges {
                                if lo <= b && b <= hi { hit = true; break; }
                            }
                            if (*neg && !hit) || (!*neg && hit) {
                                next.push(*tgt);
                            }
                        }
                        Label::Eps => {}
                    }
                }
            }

            if next.is_empty() { break; }

            next.sort_unstable();
            next.dedup();
            eps_closure(&self.states, &mut next);
            curr = next;
            i += 1;
        }

        last_accept
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

    // 既存テスト…（省略）

    #[test]
    fn digit_class() {
        // \d は 0-9、\D は それ以外
        assert!(m(r"\w+\d+\w+", "foo123bar"));
        assert!(!m(r"\d+", "12a3"));

        assert!(m(r"\D+", "abc_"));
        assert!(!m(r"\D+", "a3"));
    }

    #[test]
    fn word_class() {
        // \w は [A-Za-z0-9_]、\W は それ以外
        assert!(m(r"\w+", "Az_09"));
        assert!(!m(r"\w+", "Az-09"));

        assert!(m(r"\W", "-"));
        assert!(!m(r"\W", "A"));
        assert!(!m(r"\W", "_"));
        assert!(!m(r"\W", "5"));
    }

    #[test]
    fn space_class() {
        // \s は ASCII の空白系: space, \t, \n, \r, \x0B, \x0C
        assert!(m(r"a\s+b", "a\t b"));
        assert!(m(r"a\s*b", "a\n\nb"));
        assert!(m(r"\s", " ")); // space
        assert!(m(r"\s", "\t")); // tab
        assert!(m(r"\s", "\n")); // lf
        assert!(m(r"\s", "\r")); // cr
        assert!(m(r"\s", "\u{0B}")); // vt
        assert!(m(r"\s", "\u{0C}")); // ff

        // \S は非空白
        assert!(m(r"\S+", "abc"));
        assert!(!m(r"\S+", "a b"));
    }

    #[test]
    fn combined_preset_classes() {
        // \w+\s*\w+ パターン
        assert!(m(r"\w+\s*\w+", "hello_world"));
        assert!(m(r"\w+\s*\w+", "hello  world"));
        assert!(!m(r"\w+\s*\w+", "hello- world"));
    }

    #[test]
    fn escaped_literals() {
        // エスケープによりメタ文字をリテラルとして扱う
        assert!(m(r"\.", ".")); // dot
        assert!(m(r"\*", "*")); // star
        assert!(m(r"\+", "+")); // plus
        assert!(m(r"\?", "?")); // qmark
        assert!(m(r"\|", "|")); // alt
        assert!(m(r"\(", "(")); // lparen
        assert!(m(r"\)", ")")); // rparen
        assert!(m(r"\\", r"\")); // backslash
        assert!(m(r"\[", "[")); // lbracket
        assert!(m(r"\]", "]")); // rbracket
    }

    #[test]
    fn escaped_controls() {
        // \t \n \r は単一文字として解釈される
        assert!(m(r"a\tb", "a\tb"));
        assert!(m(r"a\nb", "a\nb"));
        assert!(m(r"a\rb", "a\rb"));
        assert!(!m(r"a\tb", "a b"));
    }
}
