// nfa.rs
use crate::error::{Error, ErrorKind, err};
use crate::token::Token;

// ===== 公開API =====

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Label {
    /// ε-遷移
    Eps,
    /// 単一バイト一致
    Byte(u8),
    /// 任意1文字（改行も含める簡易仕様）
    Any,
    /// 文字クラス（neg=true で否定）
    Class { ranges: Vec<(u8, u8)>, neg: bool },
}

#[derive(Clone, Debug)]
pub(crate) struct State {
    /// (ラベル, 遷移先) の組
    pub edges: Vec<(Label, usize)>,
}

#[derive(Clone, Debug)]
pub(crate) struct Nfa {
    pub states: Vec<State>,
    pub start: usize,
    pub accept: usize,
}

/// 後置（postfix）トークン列からNFAを構築
pub(crate) fn build_nfa(postfix: &[Token]) -> Result<Nfa, Error> {
    // 内部ビルダー：未パッチの穴を持てるように to: Option<usize>
    #[derive(Clone, Debug)]
    struct EdgeB {
        label: Label,
        to: Option<usize>,
    }
    #[derive(Clone, Debug)]
    struct StateB {
        edges: Vec<EdgeB>,
    }

    #[derive(Clone, Copy, Debug)]
    enum Slot {
        Edge(usize /*state*/, usize /*edge index*/),
    }

    #[derive(Clone, Debug)]
    struct Patch {
        slot: Slot,
    }

    #[derive(Clone, Debug)]
    struct Frag {
        start: usize,
        outs: Vec<Patch>, // 未接続の出口（穴）
    }

    impl StateB {
        fn new() -> Self {
            Self { edges: Vec::new() }
        }
        fn push_edge(&mut self, label: Label, to: Option<usize>) -> usize {
            let idx = self.edges.len();
            self.edges.push(EdgeB { label, to });
            idx
        }
    }

    fn push_state(states: &mut Vec<StateB>) -> usize {
        let id = states.len();
        states.push(StateB::new());
        id
    }

    fn patch(states: &mut [StateB], outs: &[Patch], target: usize) {
        for p in outs {
            match p.slot {
                Slot::Edge(sid, ei) => {
                    states[sid].edges[ei].to = Some(target);
                }
            }
        }
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
            Token::Class { .. } => ']', // 表示用
        }
    }

    // ===== Thompson 合成 =====
    let mut states: Vec<StateB> = Vec::new();
    let mut st: Vec<Frag> = Vec::new();

    for (i, t) in postfix.iter().cloned().enumerate() {
        match t {
            // オペランド
            Token::Char(b) => {
                let s = push_state(&mut states);
                let e_idx = states[s].push_edge(Label::Byte(b), None);
                st.push(Frag {
                    start: s,
                    outs: vec![Patch {
                        slot: Slot::Edge(s, e_idx),
                    }],
                });
            }
            Token::Dot => {
                let s = push_state(&mut states);
                let e_idx = states[s].push_edge(Label::Any, None);
                st.push(Frag {
                    start: s,
                    outs: vec![Patch {
                        slot: Slot::Edge(s, e_idx),
                    }],
                });
            }
            Token::Class { ranges, neg } => {
                let s = push_state(&mut states);
                let e_idx = states[s].push_edge(Label::Class { ranges, neg }, None);
                st.push(Frag {
                    start: s,
                    outs: vec![Patch {
                        slot: Slot::Edge(s, e_idx),
                    }],
                });
            }

            // 連接 A·B
            Token::Concat => {
                let b = st.pop().ok_or_else(|| Error {
                    kind: ErrorKind::UnexpectedToken(op_char(&t)),
                    pos: i,
                })?;
                let a = st.pop().ok_or_else(|| Error {
                    kind: ErrorKind::UnexpectedToken(op_char(&t)),
                    pos: i,
                })?;
                patch(&mut states, &a.outs, b.start);
                st.push(Frag {
                    start: a.start,
                    outs: b.outs,
                });
            }

            // 選択 A|B  : 新規Splitから A.start / B.start へ ε
            Token::Alt => {
                let b = st.pop().ok_or_else(|| Error {
                    kind: ErrorKind::UnexpectedToken(op_char(&t)),
                    pos: i,
                })?;
                let a = st.pop().ok_or_else(|| Error {
                    kind: ErrorKind::UnexpectedToken(op_char(&t)),
                    pos: i,
                })?;
                let s = push_state(&mut states);
                // ε→A.start
                states[s].push_edge(Label::Eps, Some(a.start));
                // ε→B.start
                states[s].push_edge(Label::Eps, Some(b.start));
                // outs は左右の未接続の合併
                let mut outs = a.outs;
                outs.extend_from_slice(&b.outs);
                st.push(Frag { start: s, outs });
            }

            // クリーネ閉包 A* : 新規Splitから ε→A.start / ε→(外)
            Token::Star => {
                let a = st.pop().ok_or_else(|| Error {
                    kind: ErrorKind::UnexpectedToken(op_char(&t)),
                    pos: i,
                })?;
                let s = push_state(&mut states);
                // ε→A.start
                states[s].push_edge(Label::Eps, Some(a.start));
                // ε→(外) の穴
                let out2 = states[s].push_edge(Label::Eps, None);
                // A の末端から S へ戻す
                patch(&mut states, &a.outs, s);
                st.push(Frag {
                    start: s,
                    outs: vec![Patch {
                        slot: Slot::Edge(s, out2),
                    }],
                });
            }

            // 1回以上 A+ : A の末端に Split（ε→A.start / ε→外）
            Token::Plus => {
                let a = st.pop().ok_or_else(|| Error {
                    kind: ErrorKind::UnexpectedToken(op_char(&t)),
                    pos: i,
                })?;
                let s = push_state(&mut states);
                // ε→A.start
                states[s].push_edge(Label::Eps, Some(a.start));
                // ε→(外) の穴
                let out2 = states[s].push_edge(Label::Eps, None);
                // A の末端から S へ
                patch(&mut states, &a.outs, s);
                st.push(Frag {
                    start: a.start,
                    outs: vec![Patch {
                        slot: Slot::Edge(s, out2),
                    }],
                });
            }

            // 0回または1回 A? : 新規Split（ε→A.start / ε→外）
            Token::Qmark => {
                let a = st.pop().ok_or_else(|| Error {
                    kind: ErrorKind::UnexpectedToken(op_char(&t)),
                    pos: i,
                })?;
                let s = push_state(&mut states);
                // ε→A.start
                states[s].push_edge(Label::Eps, Some(a.start));
                // ε→(外) の穴
                let out2 = states[s].push_edge(Label::Eps, None);
                // outs = A.outs + out2
                let mut outs = a.outs;
                outs.push(Patch {
                    slot: Slot::Edge(s, out2),
                });
                st.push(Frag { start: s, outs });
            }

            // 括弧は to_postfix で処理済みのはず
            Token::LParen | Token::RParen => {
                return err(ErrorKind::UnbalancedParen, i);
            }
        }
    }

    // 1つにまとまっているはず
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

    // 受理状態
    let accept = push_state(&mut states);
    // 受理にεで繋ぐ
    states[accept].edges.clear(); // 受理は遷移なし（明示的に空に）
    patch(&mut states, &top.outs, accept);

    // ===== finalize: Option<usize> を外す =====
    let mut final_states = Vec::with_capacity(states.len());
    for sb in states {
        let mut edges = Vec::with_capacity(sb.edges.len());
        for EdgeB { label, to } in sb.edges {
            let to = to.expect("unpatched edge during finalize");
            edges.push((label, to));
        }
        final_states.push(State { edges });
    }

    Ok(Nfa {
        states: final_states,
        start: top.start,
        accept,
    })
}
