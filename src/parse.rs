use crate::{
    error::{Error, ErrorKind, err},
    token::Token,
};

/// 連接が必要な箇所に `Concat` を挿入する
pub(crate) fn insert_concat(tokens: &[Token]) -> Vec<Token> {
    fn is_atom_start(t: &Token) -> bool {
        matches!(
            t,
            Token::Char(_) | Token::Dot | Token::LParen | Token::Class { .. }
        )
    }

    fn is_atom_end(t: &Token) -> bool {
        matches!(
            t,
            Token::Char(_)
            | Token::Dot
            | Token::RParen
            | Token::Class { .. }
            // 直前要素に作用した量指定子の“後ろ側”も、次が来たら連接対象になり得る
            | Token::Star
            | Token::Plus
            | Token::Qmark
        )
    }
    let mut out = Vec::with_capacity(tokens.len() * 2);
    let mut prev: Option<&Token> = None;

    for t in tokens {
        if let Some(p) = prev {
            if is_atom_end(p) && is_atom_start(t) {
                out.push(Token::Concat);
            }
        }
        out.push(t.clone());
        prev = Some(t);
    }
    out
}

/// 中置トークン列（※Concat 済み想定）を後置記法へ
pub(crate) fn to_postfix(tokens: &[Token]) -> Result<Vec<Token>, Error> {
    fn is_bin_op(t: &Token) -> bool { matches!(t, Token::Concat | Token::Alt) }
    fn precedence(op: &Token) -> u8 {
        match op {
            Token::Concat => 2,
            Token::Alt => 1,
            _ => 0,
        }
    }

    // 括弧用に (gid, mark) を持たせる。★構造体variantを明示
    #[derive(Clone, Debug)]
    enum Op {
        LParen { gid: usize, mark: usize },
        Bin(Token), // Concat / Alt
    }

    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut operator_stack: Vec<(Op, usize)> = Vec::new(); // (op, pos)

    let mut last_was_operand = false;   // 直前が「オペランド（または単項後置適用後）」か
    let mut last_was_quant   = false;   // 直前が量指定子（*,+,?）か
    let mut next_group_id: usize = 1;   // 1-origin

    for (i, t) in tokens.iter().cloned().enumerate() {
        match t {
            // ===== オペランド =====
            Token::Char(_) | Token::Dot | Token::Class { .. } => {
                out.push(t);
                last_was_operand = true;
                last_was_quant   = false;
            }

            // ===== 括弧（キャプチャ） =====
            Token::LParen => {
                let gid = next_group_id; next_group_id += 1;
                // 開いた瞬間に CapStart を出力しておく
                out.push(Token::CapStart(gid));
                // この時点の out.len() を記録（中身の有無判定に使う）
                let mark = out.len();
                operator_stack.push((Op::LParen { gid, mark }, i));
                // 直後に量指定子を許可するため operand=true にする
                last_was_operand = true;
                last_was_quant   = false;
            }
            Token::RParen => {
                // '(' まで演算子を出力
                let (gid, mark) = loop {
                    let Some((top, _pos_top)) = operator_stack.pop() else {
                        return Err(Error { kind: ErrorKind::UnbalancedParen, pos: i });
                    };
                    match top {
                        Op::LParen { gid, mark } => break (gid, mark),
                        Op::Bin(bop) => out.push(bop),
                    }
                };

                // CapStart 直後の out.len() を mark にしてある前提
                let produced = out.len().saturating_sub(mark);

                if produced == 0 {
                    // () 空グループ: CapStart の直後に CapEnd を置き、Concat で結合
                    out.push(Token::CapEnd(gid));
                    out.push(Token::Concat);
                } else {
                    // (inner) 非空: (CapStart · inner) に Concat を1本
                    out.push(Token::Concat);
                    // さらに CapEnd を置いて (… · CapEnd) に Concat
                    out.push(Token::CapEnd(gid));
                    out.push(Token::Concat);
                }

                last_was_operand = true; // () 全体で1オペランド
                last_was_quant   = false;
            }

            // ===== 単項後置（量指定子） =====
            Token::Star | Token::Plus | Token::Qmark => {
                if !last_was_operand {
                    // 例: "*a" / "|*" / "(*" など
                    return Err(Error { kind: ErrorKind::DanglingQuantifier, pos: i });
                }
                if last_was_quant {
                    // 例: "a**", "a+?" 等をエラーにする
                    return Err(Error { kind: ErrorKind::DanglingQuantifier, pos: i });
                }
                out.push(t);
                last_was_operand = true;  // 「オペランド1個分」は維持
                last_was_quant   = true;  // 直後の量指定子連鎖を禁止
            }

            // ===== 二項（左結合） =====
            Token::Concat | Token::Alt => {
                while let Some((top, _)) = operator_stack.last() {
                    match top {
                        Op::Bin(op2) if is_bin_op(op2) && precedence(op2) >= precedence(&t) => {
                            if let Some((Op::Bin(op2), _)) = operator_stack.pop() {
                                out.push(op2);
                            }
                        }
                        _ => break,
                    }
                }
                operator_stack.push((Op::Bin(t), i));
                last_was_operand = false;
                last_was_quant   = false;
            }
            // ここには来ない
            Token::CapStart(_) | Token::CapEnd(_) => {
                // 上位の tokenize/insert_concat からは来ない前提
                // 念のためエラーにしても良い
                return err(ErrorKind::UnexpectedToken('^'), i);
            }
        }
    }

    // 残りを出力
    while let Some((op, pos)) = operator_stack.pop() {
        match op {
            Op::LParen { .. } => return Err(Error { kind: ErrorKind::UnbalancedParen, pos }),
            Op::Bin(b) => out.push(b),
        }
    }

    Ok(out)
}

#[cfg(test)]
mod parse_tests {
    use super::*;
    use crate::token::tokenize;

    // --- 小道具 -------------------------------------------------------------

    /// tokenize → insert_concat → to_postfix を一気に
    fn rpn(s: &str) -> Vec<Token> {
        let t = tokenize(s).unwrap();
        let t = insert_concat(&t);
        to_postfix(&t).unwrap()
    }

    /// tokenize → insert_concat のみ
    fn with_concat(s: &str) -> Vec<Token> {
        let t = tokenize(s).unwrap();
        insert_concat(&t)
    }

    /// デバッグ・検証用: トークン列を記号化して比較しやすくする
    /// - 文字: 'c'
    /// - . : '.'
    /// - クラス: '['
    /// - 量指定子: '*', '+', '?'
    /// - 連接: '·' (中黒)
    /// - 選択: '|'
    /// - CapStart/End: 'S' / 'E'（グループIDは無視）
    fn sym(ts: &[Token]) -> String {
        use Token::*;
        ts.iter()
            .map(|t| match t {
                Char(_)      => "c",
                Dot          => ".",
                Class { .. } => "[",
                Star         => "*",
                Plus         => "+",
                Qmark        => "?",
                Concat       => "·",
                Alt          => "|",
                CapStart(_)  => "S",
                CapEnd(_)    => "E",
                LParen | RParen => unreachable!("Paren should not remain after RPN"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    // --- insert_concat のテスト ---------------------------------------------

    #[test]
    fn insert_concat_basic_between_literals() {
        use Token::*;
        let got = with_concat("ab");
        assert_eq!(got, vec![Char(b'a'), Concat, Char(b'b')]);
    }

    #[test]
    fn insert_concat_between_atom_and_group() {
        // a(b)c  →  a · ( b ) · c  （Concat が2箇所）
        use Token::*;
        let got = with_concat("a(b)c");
        // L/RParen はそのまま残り、Concat が適切に挿入されること
        assert_eq!(
            got,
            vec![Char(b'a'), Concat, LParen, Char(b'b'), RParen, Concat, Char(b'c')]
        );
    }

    #[test]
    fn insert_concat_around_quantified_atom() {
        // a* b   →  a * · b
        use Token::*;
        let got = with_concat("a*b");
        assert_eq!(got, vec![Char(b'a'), Star, Concat, Char(b'b')]);
    }

    #[test]
    fn insert_concat_class_and_dot() {
        // [0-9]. → [ · .
        let got = with_concat("[0-9].");
        assert_eq!(sym(&got), "[ · .");
    }

    // --- to_postfix（RPN） 生成のテスト -------------------------------------

    #[test]
    fn rpn_concat_has_higher_precedence_than_alt() {
        // ab|cd → a b · c d · |
        let s = sym(&rpn("ab|cd"));
        assert_eq!(s, "c c · c c · |");
    }

    #[test]
    fn rpn_quantifier_then_concat() {
        // a*b → a * b ·
        let s = sym(&rpn("a*b"));
        assert_eq!(s, "c * c ·");
    }

    #[test]
    fn rpn_group_non_empty() {
        // (ab) → S a b · · E ·
        //  内部: a b ·
        //  ) で: （CapStartとinner）に Concat、さらに CapEnd を置いて Concat
        let s = sym(&rpn("(ab)"));
        assert_eq!(s, "S c c · · E ·");
    }

    #[test]
    fn rpn_group_empty() {
        // () → S E ·
        //  空グループは CapStart の直後に CapEnd、その後に Concat 1本だけ
        let s = sym(&rpn("()"));
        assert_eq!(s, "S E ·");
    }

    #[test]
    fn rpn_group_plus_followed_by_concat() {
        // (ab)+c →
        //   S a b · · E · + c ·
        let s = sym(&rpn("(ab)+c"));
        assert_eq!(s, "S c c · · E · + c ·");
    }

    #[test]
    fn rpn_optional_group_then_concat() {
        // (a|b)?c →
        //   S a b | · E · ? c ·
        let s = sym(&rpn("(a|b)?c"));
        assert_eq!(s, "S c c | · E · ? c ·");
    }

    #[test]
    fn rpn_class_and_dot_concat() {
        // [0-9]. → [ . ·
        let s = sym(&rpn("[0-9]."));
        assert_eq!(s, "[ . ·");
    }

    // --- エラーパス ---------------------------------------------------------

    #[test]
    fn rpn_error_on_dangling_quantifier_prefix() {
        // "*a" はトークン直前がオペランドでない量指定子なのでエラー
        let t = tokenize("*a").unwrap();
        let t = insert_concat(&t);
        let err = to_postfix(&t).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::DanglingQuantifier));
    }

    #[test]
    fn rpn_error_on_dangling_quantifier_chain() {
        // "a**" の2つ目の * は直前が量指定子なのでエラー
        let t = tokenize("a**").unwrap();
        let t = insert_concat(&t);
        let err = to_postfix(&t).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::DanglingQuantifier));
    }

    #[test]
    fn rpn_error_on_unbalanced_paren_leftover() {
        // "(ab" は閉じていないので UnbalancedParen
        let t = tokenize("(ab").unwrap();
        let t = insert_concat(&t);
        let err = to_postfix(&t).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnbalancedParen));
    }

    #[test]
    fn rpn_error_on_unexpected_cap_tokens() {
        // 実装は CapStart/CapEnd が入力に来たら UnexpectedToken を返す
        // ここでは直接 to_postfix に流し込んで確認する
        use Token::*;
        let err = to_postfix(&[CapStart(1)]).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken(_)));

        let err = to_postfix(&[CapEnd(1)]).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken(_)));
    }

    // --- 参考: 既存テストに近い形（順序を contains ではなく完全一致で） -----

    #[test]
    fn rpn_of_group_plus_strict() {
        // (ab)+ → S a b · · E · +
        let s = sym(&rpn("(ab)+"));
        assert_eq!(s, "S c c · · E · +");
    }
}
