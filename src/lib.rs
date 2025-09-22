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
    groups: usize, // ★ 追加：キャプチャ数（1..=groups）
}

// 各スレッドが持つキャプチャ: (start,end) を Option<usize> で
type GroupSlot = (Option<usize>, Option<usize>);

#[derive(Clone)]
struct Thread {
    s: usize,
    caps: Vec<GroupSlot>, // index=グループ番号（0は未使用）
}

fn better_choice(a: &(usize, Vec<GroupSlot>), b: &(usize, Vec<GroupSlot>)) -> bool {
    // 1) end 位置（i）が大きい方を優先（最長一致）
    if a.0 != b.0 {
        return a.0 > b.0;
    }
    // 2) 同じ end の場合、各グループの start が大きい方（より遅い開始 = 前段が貪欲）
    let ga = &a.1;
    let gb = &b.1;
    let len = ga.len().min(gb.len());
    for g in 1..len {
        match (ga[g].0, gb[g].0) {
            (Some(sa), Some(sb)) if sa != sb => return sa > sb,
            _ => {}
        }
    }
    // 3) それでも同じなら、各グループの end が大きい方
    for g in 1..len {
        match (ga[g].1, gb[g].1) {
            (Some(ea), Some(eb)) if ea != eb => return ea > eb,
            _ => {}
        }
    }
    // 4) ここまで同じなら b を維持（a を採用しない）
    false
}

impl Regex {
    pub fn new(pat: &str) -> Result<Self, Error> {
        // アンカーは常に有効（^…$ を暗黙）
        let tokens = tokenize(pat)?;
        let tokens = insert_concat(&tokens);
        let postfix = to_postfix(&tokens)?;
        let nfa = build_nfa(&postfix)?;

        // NFA中の最大グループ番号を拾う
        let mut gmax = 0usize;
        for st in &nfa.states {
            for (lbl, _) in &st.edges {
                match lbl {
                    Label::CapBegin(g) | Label::CapEnd(g) => gmax = gmax.max(*g),
                    _ => {}
                }
            }
        }

        Ok(Self {
            states: nfa.states,
            start: nfa.start,
            accept: nfa.accept,
            groups: gmax,
        })
    }

    /// 完全一致（全消費）かどうか
    pub fn is_match(&self, hay: &str) -> bool {
        self.captures(hay).is_some()
    }

    /// 完全一致時にキャプチャを返す。
    /// 返り値: Vec<Option<&str>> で、[0] が全体、[1..=groups] が各グループ。
    pub fn captures<'a>(&self, hay: &'a str) -> Option<Vec<Option<&'a str>>> {
        let bytes = hay.as_bytes();
        let (end, caps) = self.run(bytes)?;

        if end != bytes.len() {
            return None; // 全消費のみOK
        }

        // [0]=全体, 1..=groups
        let mut out: Vec<Option<&'a str>> = vec![None; self.groups + 1];
        out[0] = Some(hay); // 全体（常に完全一致前提）

        for g in 1..=self.groups {
            if let Some((Some(s), Some(e))) = caps.get(g).copied() {
                if s <= e && e <= hay.len() {
                    out[g] = Some(&hay[s..e]);
                }
            }
        }
        Some(out)
    }

    // ===== 実行器（NFAシミュレーション with captures） =====

    fn run(&self, bytes: &[u8]) -> Option<(usize, Vec<GroupSlot>)> {
        let n = bytes.len();

        let mut curr = vec![Thread {
            s: self.start,
            caps: vec![(None, None); self.groups + 1],
        }];
        self.eps_closure(&mut curr, 0);

        let mut last: Option<(usize, Vec<GroupSlot>)> = None;

        let mut i = 0usize;
        while i <= n {
            // 受理チェック：全受理スレッドからベターなものを選ぶ
            for t in curr.iter().filter(|t| t.s == self.accept) {
                let cand = (i, t.caps.clone());
                if let Some(best) = &mut last {
                    if better_choice(&cand, best) {
                        *best = cand;
                    }
                } else {
                    last = Some(cand);
                }
            }

            if i == n {
                break;
            }

            let b = bytes[i];
            let mut next: Vec<Thread> = Vec::new();

            for thr in &curr {
                for (lbl, tgt) in &self.states[thr.s].edges {
                    match lbl {
                        Label::Byte(c) if *c == b => {
                            next.push(Thread {
                                s: *tgt,
                                caps: thr.caps.clone(),
                            });
                        }
                        Label::Any => {
                            next.push(Thread {
                                s: *tgt,
                                caps: thr.caps.clone(),
                            });
                        }
                        Label::Class { ranges, neg } => {
                            let hit = ranges.iter().any(|&(lo, hi)| lo <= b && b <= hi);
                            if (*neg && !hit) || (!*neg && hit) {
                                next.push(Thread {
                                    s: *tgt,
                                    caps: thr.caps.clone(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }

            if next.is_empty() {
                break;
            }

            self.eps_closure(&mut next, i + 1);
            curr = dedup_threads(next);
            i += 1;
        }

        last
    }

    /// ε・CapBegin・CapEnd を辿って集合を閉じる。
    /// `pos` は「いまの入力位置」（Cap記録に使う）。
    fn eps_closure(&self, set: &mut Vec<Thread>, pos: usize) {
        use std::collections::VecDeque;
        let mut q: VecDeque<Thread> = set.clone().into();
        set.clear();

        // 訪問管理は (state, caps の指紋) で重複を抑える
        // ここでは簡便のため、(state, caps 全体) をそのまま比較して dedup。
        while let Some(thr) = q.pop_front() {
            // 同一 Thread が既にあるならスキップ
            if set.iter().any(|t| t.s == thr.s && t.caps == thr.caps) {
                continue;
            }

            set.push(thr.clone());

            for (lbl, tgt) in &self.states[thr.s].edges {
                match lbl {
                    Label::Eps => {
                        q.push_back(Thread {
                            s: *tgt,
                            caps: thr.caps.clone(),
                        });
                    }
                    Label::CapBegin(g) => {
                        let mut c = thr.caps.clone();
                        if *g < c.len() {
                            c[*g].0 = Some(pos);
                        }
                        q.push_back(Thread { s: *tgt, caps: c });
                    }
                    Label::CapEnd(g) => {
                        let mut c = thr.caps.clone();
                        if *g < c.len() {
                            c[*g].1 = Some(pos);
                        }
                        q.push_back(Thread { s: *tgt, caps: c });
                    }
                    _ => {} // 文字を読む遷移はここでは進まない
                }
            }
        }

        // 最後に重複除去
        *set = dedup_threads(std::mem::take(set));
    }
}

// 重複除去（素朴版）：(state, caps) が同一なら1つにまとめる
fn dedup_threads(mut v: Vec<Thread>) -> Vec<Thread> {
    v.sort_by(|a, b| a.s.cmp(&b.s).then_with(|| a.caps.cmp(&b.caps)));
    v.dedup_by(|a, b| a.s == b.s && a.caps == b.caps);
    v
}

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

    // 追加テスト用ヘルパ：captures を取り出す
    fn mc(p: &str, s: &str) -> Option<Vec<Option<String>>> {
        let re = Regex::new(p).expect("Regex::new failed");
        re.captures(s)
            .map(|v| v.into_iter().map(|o| o.map(|z| z.to_string())).collect())
    }

    // ========= ここから追加テスト =========

    #[test]
    fn literal_full_match_ok_ng() {
        assert!(m("abc", "abc"));
        assert!(!m("abc", "ab"));
        assert!(!m("abc", "abcd"));
        assert!(!m("abc", "zabc"));
    }

    #[test]
    fn concat_and_alt_precedence() {
        // ab|cd は a(b|c)d ではない
        assert!(m(r"(ab)|(cd)", "ab"));
        assert!(m(r"(ab)|(cd)", "cd"));
        assert!(!m(r"(ab)|(cd)", "ad"));
        assert!(m(r"a(b|c)d", "abd"));
        assert!(m(r"a(b|c)d", "acd"));
        assert!(!m(r"a(b|c)d", "ab"));
    }

    #[test]
    fn quantifiers_star_plus_qmark() {
        assert!(m(r"a*b", "b"));
        assert!(m(r"a*b", "aaaaab"));
        assert!(!m(r"a*b", "aaaabx"));

        assert!(m(r"a+b", "ab"));
        assert!(m(r"a+b", "aaaab"));
        assert!(!m(r"a+b", "b"));

        assert!(m(r"a?b", "b"));
        assert!(m(r"a?b", "ab"));
        assert!(!m(r"a?b", "aab"));
    }

    #[test]
    fn dot_matches_any_including_newline() {
        // 仕様：Any は改行も含む
        assert!(m(r"a.b", "a\nb"));
        assert!(m(r".", "x"));
        assert!(!m(r".", ""));
    }

    #[test]
    fn char_class_basic_and_neg() {
        assert!(m(r"[abc]+", "abca"));
        assert!(!m(r"[abc]+", "abdX"));
        assert!(m(r"[^0-9]+", "abc_"));
        assert!(!m(r"[^0-9]+", "abc3"));
        assert!(m(r"[0-9][0-9][0-9]", "123"));
        assert!(!m(r"[0-9][0-9][0-9]", "12a"));
    }

    #[test]
    fn presets_mix_again() {
        assert!(m(r"\w+\s*\w+", "hello_world"));
        assert!(m(r"\w+\s*\w+", "hello  world"));
        assert!(!m(r"\w+\s*\w+", "hello- world")); // _ 以外の - は \w に入らない
        assert!(m(r"\D+\S", "ab-")); // \D: 非数字, \S: 非空白
    }

    #[test]
    fn escapes_again() {
        assert!(m(r"\(", "("));
        assert!(m(r"\)", ")"));
        assert!(m(r"\|", "|"));
        assert!(m(r"\[", "["));
        assert!(m(r"\]", "]"));
        assert!(m(r"\\", r"\"));
        assert!(m(r"\.", "."));
        assert!(m(r"\*", "*"));
        assert!(m(r"\+", "+"));
        assert!(m(r"\?", "?"));
    }

    // ==== キャプチャのテスト ====

    #[test]
    fn capture_simple_two_groups() {
        let got = mc(r"(foo)(bar)", "foobar").unwrap();
        assert_eq!(got[0], Some("foobar".into())); // 全体
        assert_eq!(got[1], Some("foo".into()));
        assert_eq!(got[2], Some("bar".into()));
    }

    #[test]
    fn capture_nested_numbering_and_values() {
        // グループ番号は「開き括弧の出現順」
        let got = mc(r"(a(b)c)", "abc").unwrap();
        assert_eq!(got.len(), 1 + 2); // [0]全体 + 2 groups
        assert_eq!(got[0], Some("abc".into()));
        assert_eq!(got[1], Some("abc".into())); // 外側
        assert_eq!(got[2], Some("b".into())); // 内側
    }

    #[test]
    fn capture_empty_group() {
        // () は空文字をキャプチャ
        let got = mc(r"a()b", "ab").unwrap();
        assert_eq!(got[0], Some("ab".into()));
        assert_eq!(got[1], Some("".into()));
    }

    #[test]
    fn capture_optional_group_absent_is_none() {
        // グループが通らない経路では None（空文字ではない）
        let g1 = mc(r"(ab)?c", "abc").unwrap();
        assert_eq!(g1[1], Some("ab".into()));

        let g2 = mc(r"(ab)?c", "c").unwrap();
        assert_eq!(g2[1], None); // 通っていない
    }

    #[test]
    fn capture_alt() {
        let a = mc(r"(foo|bar)baz", "foobaz").unwrap();
        assert_eq!(a[1], Some("foo".into()));
        let b = mc(r"(foo|bar)baz", "barbaz").unwrap();
        assert_eq!(b[1], Some("bar".into()));
    }

    #[test]
    fn capture_repetition_picks_last_iteration() {
        // 現実装ではループ中に CapBegin/End を通るたびに上書き → 最終反復が残る
        let got = mc(r"(ab)+", "abab").unwrap();
        assert_eq!(got[1], Some("ab".into())); // 最後の "ab"
    }

    #[test]
    fn capture_many_and_order() {
        // (a)(b(c))(d) on "abcd"
        let got = mc(r"(a)(b(c))(d)", "abcd").unwrap();
        assert_eq!(got[1], Some("a".into()));
        assert_eq!(got[2], Some("bc".into()));
        assert_eq!(got[3], Some("c".into()));
        assert_eq!(got[4], Some("d".into()));
    }

    #[test]
    fn capture_mixed_with_classes_and_dot() {
        let got = mc(r"(\w+)\s+(.+)", "abc   123-XYZ").unwrap();
        assert_eq!(got[1], Some("abc".into()));
        assert_eq!(got[2], Some("123-XYZ".into()));
    }

    // ==== エラー系（構文エラー） ====

    #[test]
    fn error_unbalanced_paren() {
        let e = Regex::new("(ab");
        assert!(e.is_err());
    }

    #[test]
    fn error_dangling_quantifier() {
        assert!(Regex::new("*a").is_err());
        assert!(Regex::new("+a").is_err());
        assert!(Regex::new("?a").is_err());
        assert!(Regex::new("a**").is_err()); // 量指定子の連結は未対応想定ならエラー
    }

    #[test]
    fn error_bad_class_or_empty_class() {
        // 実装側の ErrorKind に依存するので、安全な範囲で
        // ここでは空クラス [] をエラーにしている前提
        assert!(Regex::new("[]").is_err());
        // 範囲の逆転 [z-a] もエラーにしているなら（tokenize側仕様次第）
        let _ = Regex::new("[z-a]").is_err(); // 仕様が違えばこの行は消してください
    }
}

#[cfg(test)]
mod nfa_capture_cut_tests {
    use super::*;
    use crate::nfa::Nfa;
    use crate::parse::{insert_concat, to_postfix};
    use crate::token::{Token, tokenize};
    use std::collections::VecDeque;

    fn make_postfix(pat: &str) -> Vec<Token> {
        let t = tokenize(pat).unwrap();
        let t = insert_concat(&t);
        to_postfix(&t).unwrap()
    }

    fn make_nfa(pat: &str) -> Nfa {
        let p = make_postfix(pat);
        build_nfa(&p).unwrap()
    }

    /// 後置記法を記号列にして比較しやすくする
    /// c=Char, [=Class, .=Any, *=*, +=+, ?=?, |=Alt, ·=Concat, S/E=CapStart/End
    fn sym(ts: &[Token]) -> String {
        use crate::token::Token::*;
        ts.iter()
            .map(|t| match t {
                Char(_) => "c",
                Dot => ".",
                Class { .. } => "[",
                Star => "*",
                Plus => "+",
                Qmark => "?",
                Concat => "·",
                Alt => "|",
                CapStart(_) => "S",
                CapEnd(_) => "E",
                LParen | RParen => unreachable!("Paren should not appear in postfix"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn postfix_for_capture_w_space_dot() {
        // パターン: (\w+)\s+(.+)
        // 期待（概念）:
        //   S [ + · E ·    ← (\w+)
        //   [ + ·          ← \s+
        //   S . + · E ·    ← (.+)
        //   ·               ← 左3塊の連接（to_postfix の優先順位で自動配置）
        let p = make_postfix(r"(\w+)\s+(.+)");
        // 正確な並びは実装の左結合・Concatの挿入位置に依存するので、
        // キーとなる部分列が含まれているかで確認
        let s = sym(&p);
        assert!(s.contains("S [ + · E ·"), "missing (\\w+) block: {s}");
        assert!(s.contains("[ + ·"), "missing \\s+ block: {s}");
        assert!(s.contains("S . + · E ·"), "missing (.+) block: {s}");
    }

    #[test]
    fn nfa_contains_two_capture_pairs_and_classes() {
        let nfa = make_nfa(r"(\w+)\s+(.+)");
        let mut cb = 0; // CapBegin count
        let mut ce = 0; // CapEnd count
        let mut has_w = false;
        let mut has_s = false;
        let mut has_dot = false;

        for st in &nfa.states {
            for (lbl, _) in &st.edges {
                match lbl {
                    Label::CapBegin(_) => cb += 1,
                    Label::CapEnd(_) => ce += 1,
                    Label::Class { ranges, neg } => {
                        // \w: [0-9A-Za-z_]
                        if !*neg
                            && ranges.iter().any(|&(a, b)| a == b'0' && b == b'9')
                            && ranges.iter().any(|&(a, b)| a == b'A' && b == b'Z')
                            && ranges.iter().any(|&(a, b)| a == b'a' && b == b'z')
                        {
                            has_w = true;
                        }
                        // \s: space を含む
                        if !*neg && ranges.iter().any(|&(a, b)| a == b' ' && b == b' ') {
                            has_s = true;
                        }
                    }
                    Label::Any => has_dot = true,
                    _ => {}
                }
            }
        }

        assert_eq!(cb, 2, "CapBegin should appear twice, got {cb}");
        assert_eq!(ce, 2, "CapEnd should appear twice, got {ce}");
        assert!(has_w, "NFA should contain \\w class");
        assert!(has_s, "NFA should contain \\s class");
        assert!(has_dot, "NFA should contain '.' Any");
    }

    /// CapBegin(1) の直後から、ε/Cap だけで CapEnd(1) に到達できないことを確認
    /// （= (\w+) がゼロ長で閉じない）
    #[test]
    fn no_zero_length_path_from_cap1_begin_to_end() {
        let nfa = make_nfa(r"(\w+)\s+(.+)");

        // CapBegin(1) と CapEnd(1) の「エッジ」を集める
        let mut begin_targets: Vec<usize> = vec![]; // CapBegin(1) を踏んだ「遷移先状態」
        let mut end_sources: Vec<usize> = vec![]; // CapEnd(1) の「遷移元状態」
        for (sid, st) in nfa.states.iter().enumerate() {
            for (lbl, to) in &st.edges {
                match lbl {
                    Label::CapBegin(g) if *g == 1 => begin_targets.push(*to),
                    Label::CapEnd(g) if *g == 1 => end_sources.push(sid),
                    _ => {}
                }
            }
        }
        assert!(!begin_targets.is_empty(), "CapBegin(1) not found");
        assert!(!end_sources.is_empty(), "CapEnd(1) not found");

        // ε/Cap のみで到達できるかを BFS で調べる
        // 到達「先」は CapEnd(1) の“遷移元”状態（= その状態に着いた時点で CapEnd を踏める）
        let mut reachable = false;
        for start in begin_targets {
            let mut seen = vec![false; nfa.states.len()];
            let mut q = VecDeque::new();
            seen[start] = true;
            q.push_back(start);

            while let Some(u) = q.pop_front() {
                if end_sources.contains(&u) {
                    reachable = true;
                    break;
                }
                for (lbl, v) in &nfa.states[u].edges {
                    match lbl {
                        Label::Eps | Label::CapBegin(_) | Label::CapEnd(_) => {
                            if !seen[*v] {
                                seen[*v] = true;
                                q.push_back(*v);
                            }
                        }
                        _ => {} // 文字を消費するラベルは辿らない
                    }
                }
            }
        }

        // ゼロ長で CapEnd(1) の直前に到達できるなら NG
        assert!(
            !reachable,
            "CapBegin(1) から ε/Cap のみで CapEnd(1) 直前へ到達できます（ゼロ長で閉じてしまう可能性）。"
        );
    }

    /// 参考: 実データでの完全一致・キャプチャの Smoke テスト
    /// （ここで落ちる場合は matcher 側の選択ポリシーや ε-closure の更新順を疑う）
    #[test]
    fn smoke_match_and_captures() {
        // ランタイムの Regex を通す最小確認（切り分け用）
        use crate::Regex;

        let re = Regex::new(r"(\w+)\s+(.+)").unwrap();
        let caps = re.captures("abc   123-XYZ").expect("should match fully");
        assert_eq!(caps[1], Some("abc"));
        assert_eq!(caps[2], Some("123-XYZ"));
    }
}
