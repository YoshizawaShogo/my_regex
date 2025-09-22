// nfa.rs
use crate::error::{Error, ErrorKind, err};
use crate::token::Token;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Label {
    Eps,
    Byte(u8),
    Any,
    Class { ranges: Vec<(u8, u8)>, neg: bool },
    CapBegin(usize),
    CapEnd(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct State {
    pub edges: Vec<(Label, usize)>,
}

#[derive(Clone, Debug)]
pub(crate) struct Nfa {
    pub states: Vec<State>,
    pub start: usize,
    pub accept: usize,
}

pub(crate) fn build_nfa(postfix: &[Token]) -> Result<Nfa, Error> {
    // ===== 内部ビルダー（未パッチの to を持つ） =====
    #[derive(Clone, Debug)]
    struct EdgeBuilder {
        label: Label,
        to: Option<usize>,
    }
    #[derive(Clone, Debug)]
    struct StateBuilder {
        edges: Vec<EdgeBuilder>,
    }

    // 「穴」を (state_id, edge_index) の2-tupleで表す
    type Hole = (usize, usize);

    #[derive(Clone, Debug)]
    struct Frag {
        start: usize,
        outs: Vec<Hole>,
    }

    impl StateBuilder {
        fn new() -> Self {
            Self { edges: Vec::new() }
        }
        fn add_edge(&mut self, label: Label, to: Option<usize>) -> usize {
            let idx = self.edges.len();
            self.edges.push(EdgeBuilder { label, to });
            idx
        }
    }

    // === ヘルパ ===
    fn new_state(states: &mut Vec<StateBuilder>) -> usize {
        let id = states.len();
        states.push(StateBuilder::new());
        id
    }

    fn hole(states: &mut [StateBuilder], sid: usize, label: Label) -> Hole {
        // to=None のエッジを1本作って穴として返す
        let ei = states[sid].add_edge(label, None);
        (sid, ei)
    }

    fn edge_to(states: &mut [StateBuilder], sid: usize, label: Label, to: usize) {
        states[sid].add_edge(label, Some(to));
    }

    fn patch(states: &mut [StateBuilder], holes: &[Hole], target: usize) {
        for &(sid, ei) in holes {
            states[sid].edges[ei].to = Some(target);
        }
    }

    fn pop1<T: Clone>(st: &mut Vec<T>, i: usize, t: &Token) -> Result<T, Error> {
        st.pop().ok_or_else(|| Error {
            kind: ErrorKind::UnexpectedToken(op_char(t)),
            pos: i,
        })
    }
    fn pop2<T: Clone>(st: &mut Vec<T>, i: usize, t: &Token) -> Result<(T, T), Error> {
        let b = pop1(st, i, t)?;
        let a = pop1(st, i, t)?;
        Ok((a, b))
    }

    fn op_char(t: &Token) -> char {
        match t {
            Token::Alt => '|',
            Token::Concat => '·',
            Token::Star => '*',
            Token::Plus => '+',
            Token::Qmark => '?',
            Token::LParen => '(',
            Token::RParen => ')',
            Token::Dot => '.',
            Token::Char(c) => *c as char,
            Token::Class { .. } => ']',
            Token::CapStart(_gid) => '(',
            Token::CapEnd(_gid) => ')',
        }
    }

    // 単一オペランドから 1本エッジの Frag を作る
    fn make_unary_frag(states: &mut Vec<StateBuilder>, label: Label) -> Frag {
        let s = new_state(states);
        let h = hole(states.as_mut_slice(), s, label);
        Frag {
            start: s,
            outs: vec![h],
        }
    }

    // ===== Thompson 合成本体 =====
    let mut states: Vec<StateBuilder> = Vec::new();

    // グローバル start を 0 に固定（先に 0 を作っておく）
    let global_start = new_state(&mut states);

    let mut st: Vec<Frag> = Vec::new();

    for (i, t) in postfix.iter().enumerate() {
        match t {
            // オペランド
            Token::Char(b) => st.push(make_unary_frag(&mut states, Label::Byte(*b))),
            Token::Dot => st.push(make_unary_frag(&mut states, Label::Any)),
            Token::Class { ranges, neg } => {
                st.push(make_unary_frag(
                    &mut states,
                    Label::Class {
                        ranges: ranges.clone(),
                        neg: *neg,
                    },
                ));
            }

            // A · B
            Token::Concat => {
                let (a, b) = pop2(&mut st, i, t)?;
                // A.outs を B.start にパッチ
                patch(&mut states, &a.outs, b.start);
                st.push(Frag {
                    start: a.start,
                    outs: b.outs,
                });
            }

            // A | B
            Token::Alt => {
                let (a, b) = pop2(&mut st, i, t)?;
                let s = new_state(&mut states);
                edge_to(&mut states, s, Label::Eps, a.start);
                edge_to(&mut states, s, Label::Eps, b.start);
                let mut outs = a.outs;
                outs.extend_from_slice(&b.outs);
                st.push(Frag { start: s, outs });
            }

            // A*
            Token::Star => {
                let a = pop1(&mut st, i, t)?;
                let s = new_state(&mut states);
                // ε->A.start と ε->外（穴）
                edge_to(&mut states, s, Label::Eps, a.start);
                let h = hole(&mut states, s, Label::Eps);
                // A の末端から S へ戻す
                patch(&mut states, &a.outs, s);
                st.push(Frag {
                    start: s,
                    outs: vec![h],
                });
            }

            // A+  (A の末尾から Split)
            Token::Plus => {
                let a = pop1(&mut st, i, t)?;
                let s = new_state(&mut states);
                edge_to(&mut states, s, Label::Eps, a.start);
                let h = hole(&mut states, s, Label::Eps);
                patch(&mut states, &a.outs, s);
                // start は A を保つ（最低1回）
                st.push(Frag {
                    start: a.start,
                    outs: vec![h],
                });
            }

            // A?
            Token::Qmark => {
                let a = pop1(&mut st, i, t)?;
                let s = new_state(&mut states);
                edge_to(&mut states, s, Label::Eps, a.start);
                let h = hole(&mut states, s, Label::Eps);
                let mut outs = a.outs;
                outs.push(h);
                st.push(Frag { start: s, outs });
            }
            Token::CapStart(gid) => {
                st.push(make_unary_frag(&mut states, Label::CapBegin(*gid)));
            }
            Token::CapEnd(gid) => {
                st.push(make_unary_frag(&mut states, Label::CapEnd(*gid)));
            }

            // 括弧は postfix 済みの前提
            Token::LParen | Token::RParen => return err(ErrorKind::UnbalancedParen, i),
        }
    }

    let top = st.pop().ok_or_else(|| Error {
        kind: ErrorKind::UnexpectedToken('$'),
        pos: postfix.len(),
    })?;
    if !st.is_empty() {
        return Err(Error {
            kind: ErrorKind::UnexpectedToken('$'),
            pos: postfix.len(),
        });
    }

    // 受理状態を作り、未パッチを受理へ
    let accept = new_state(&mut states);
    patch(&mut states, &top.outs, accept);

    // グローバル start(=0) から top.start へ ε（top が 0 でなければ）
    if top.start != global_start {
        edge_to(&mut states, global_start, Label::Eps, top.start);
    }

    // ===== finalize: Option<usize> を外す =====
    let mut final_states = Vec::with_capacity(states.len());
    for sb in states {
        let mut edges = Vec::with_capacity(sb.edges.len());
        for EdgeBuilder { label, to } in sb.edges {
            let to = to.expect("unpatched edge during finalize");
            edges.push((label, to));
        }
        final_states.push(State { edges });
    }

    Ok(Nfa {
        states: final_states,
        start: global_start, // ★ start=0 固定
        accept,
    })
}

#[cfg(test)]
mod nfa_tests {
    use super::*;
    use crate::parse::{insert_concat, to_postfix};
    use crate::token::tokenize;

    fn make_nfa(pat: &str) -> Nfa {
        let t = tokenize(pat).unwrap();
        let t = insert_concat(&t);
        let p = to_postfix(&t).unwrap();
        build_nfa(&p).unwrap()
    }

    fn labels(nfa: &Nfa, sid: usize) -> Vec<String> {
        nfa.states[sid]
            .edges
            .iter()
            .map(|(l, _)| match l {
                Label::Eps => "ε".to_string(),
                Label::Byte(b) => format!("{}", *b as char),
                Label::Any => ".".to_string(),
                Label::Class { .. } => "[]".to_string(),
                Label::CapBegin(g) => format!("S{}", g),
                Label::CapEnd(g) => format!("E{}", g),
            })
            .collect()
    }

    #[test]
    fn literal_nfa() {
        let nfa = make_nfa("a");
        assert_eq!(nfa.start, 0);
        assert!(nfa.accept < nfa.states.len());
        // start から 'a' で accept へ遷移
        let lbls = labels(&nfa, nfa.start);
        assert_eq!(lbls, vec!["ε"]);
    }

    #[test]
    fn concat_nfa() {
        let nfa = make_nfa("ab");
        // 2文字なので、中間状態を経由
        let mut seen = false;
        for st in &nfa.states {
            if st.edges.iter().any(|(l, _)| matches!(l, Label::Byte(b'a'))) {
                seen = true;
            }
        }
        assert!(seen, "should contain edge labeled 'a'");
    }

    #[test]
    fn alt_nfa() {
        let nfa = make_nfa("a|b");
        // global start の次のノードに ε が2本あるはず
        let (_, to) = &nfa.states[nfa.start].edges[0];
        let lbls = labels(&nfa, *to);
        assert_eq!(lbls, vec!["ε", "ε"]);
    }

    #[test]
    fn star_nfa() {
        let nfa = make_nfa("a*");
        // star 構造なので、εループが含まれる
        let has_loop = nfa.states.iter().any(|st| {
            st.edges
                .iter()
                .any(|(l, to)| matches!(l, Label::Eps) && st.edges.iter().any(|(_, t2)| t2 == to))
        });
        assert!(has_loop, "should contain epsilon loop");
    }

    #[test]
    fn plus_nfa() {
        let nfa = make_nfa("a+");
        // プラスなので、a が最低1回は現れる
        let has_a = nfa
            .states
            .iter()
            .any(|st| st.edges.iter().any(|(l, _)| matches!(l, Label::Byte(b'a'))));
        assert!(has_a);
    }

    #[test]
    fn qmark_nfa() {
        let nfa = make_nfa("a?");
        // optional: ε で飛ばせる分岐があるはず
        let start_lbls = labels(&nfa, nfa.start);
        assert!(start_lbls.contains(&"ε".to_string()));
    }

    #[test]
    fn class_and_dot_nfa() {
        let nfa = make_nfa("[0-9].");
        let mut seen_dot = false;
        let mut seen_class = false;
        for st in &nfa.states {
            for (l, _) in &st.edges {
                match l {
                    Label::Any => seen_dot = true,
                    Label::Class { .. } => seen_class = true,
                    _ => {}
                }
            }
        }
        assert!(seen_dot && seen_class);
    }

    #[test]
    fn capture_group_nfa() {
        let nfa = make_nfa("(ab)");
        // CapBegin と CapEnd が含まれるか
        let mut has_begin = false;
        let mut has_end = false;
        for st in &nfa.states {
            for (l, _) in &st.edges {
                match l {
                    Label::CapBegin(_) => has_begin = true,
                    Label::CapEnd(_) => has_end = true,
                    _ => {}
                }
            }
        }
        assert!(has_begin && has_end);
    }

    #[test]
    fn error_on_empty_postfix() {
        // build_nfa は空入力で UnexpectedToken を返す
        let err = build_nfa(&[]).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken(_)));
    }
}
