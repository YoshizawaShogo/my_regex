use crate::{
    error::{Error, ErrorKind, err},
    token::Token,
};

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

/// 連接が必要な箇所に `Concat` を挿入する
pub(crate) fn insert_concat(tokens: &[Token]) -> Vec<Token> {
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

fn is_bin_op(t: &Token) -> bool {
    matches!(t, Token::Concat | Token::Alt)
}

fn precedence(op: &Token) -> u8 {
    match op {
        // 後置単項演算子はここではスタックに積まない（そのまま出力）ので未使用
        // 値が大きい方が優先度が高い
        Token::Concat => 2, // 連接
        Token::Alt => 1,    // |
        _ => 0,
    }
}

/// 中置トークン列（※Concat 済み想定）を後置記法へ
pub(crate) fn to_postfix(tokens: &[Token]) -> Result<Vec<Token>, Error> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut operator_stack: Vec<(Token, usize)> = Vec::new(); // (op, pos)
    let mut last_was_operand = false; // 直前がオペランド（または単項後置演算適用後）なら true

    for (i, t) in tokens.iter().cloned().enumerate() {
        match t {
            // オペランド類
            Token::Char(_) | Token::Dot | Token::Class { .. } => {
                out.push(t);
                last_was_operand = true;
            }
            // 括弧
            Token::LParen => {
                operator_stack.push((Token::LParen, i));
                last_was_operand = false;
            }
            Token::RParen => {
                let mut found_lparen = false;
                // '(' まで演算子を出力
                while let Some((top, _pos)) = operator_stack.pop() {
                    if matches!(top, Token::LParen) {
                        found_lparen = true;
                        break;
                    }
                    out.push(top);
                }
                // '(' が無かった
                if !found_lparen {
                    return err(ErrorKind::UnbalancedParen, i);
                }
                last_was_operand = true; // ( … ) はひとかたまりのオペランド扱い
            }

            // 単項後置演算子（その場で出力末尾に付ける）
            Token::Star | Token::Plus | Token::Qmark => {
                if !last_was_operand {
                    // 例: "*a" / "|*" / "(*" など
                    return err(ErrorKind::DanglingQuantifier, i);
                }
                out.push(t);
                // 量指定子のあとも依然「オペランドが存在する」状態
                last_was_operand = true;
            }

            // 二項演算子（Concat / Alt）
            Token::Concat | Token::Alt => {
                // 左結合なので、優先度が「高い or 等しい」ものを先に吐く
                while let Some((top, _pos)) = operator_stack.last() {
                    if is_bin_op(top) && precedence(top) >= precedence(&t) {
                        out.push(top.clone());
                        operator_stack.pop();
                    } else {
                        break;
                    }
                }
                operator_stack.push((t, i));
                last_was_operand = false; // この後は右側オペランドを期待
            }
        }
    }

    // 残りの演算子を出力
    while let Some((op, pos)) = operator_stack.pop() {
        if matches!(op, Token::LParen | Token::RParen) {
            return err(ErrorKind::UnbalancedParen, pos);
        }
        out.push(op);
    }

    Ok(out)
}
