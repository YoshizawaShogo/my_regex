use crate::error::{Error, ErrorKind, err};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Char(u8), // literal byte
    Dot,      // .
    LParen,   // (
    RParen,   // )
    Alt,      // |
    Star,     // *
    Plus,     // +
    Qmark,    // ?
    Class { ranges: Vec<(u8, u8)>, neg: bool },
    Concat, // implicit concatenation
}

// ===== Lexer =====
// 追記: プリセットクラスの定義
fn predefined_class(esc: u8) -> Option<(Vec<(u8, u8)>, bool)> {
    // 小文字が肯定、対応する大文字が否定
    match esc {
        b'd' => Some((vec![(b'0', b'9')], false)),
        b'D' => Some((vec![(b'0', b'9')], true)),

        // \s は Unicode だと広いが、ここでは ASCII 的に
        // space, \t, \n, \r, \x0B (VT), \x0C (FF)
        b's' => Some((
            vec![
                (b' ', b' '),
                (b'\t', b'\t'),
                (b'\n', b'\n'),
                (b'\r', b'\r'),
                (0x0B, 0x0B),
                (0x0C, 0x0C),
            ],
            false,
        )),
        b'S' => Some((
            vec![
                (b' ', b' '),
                (b'\t', b'\t'),
                (b'\n', b'\n'),
                (b'\r', b'\r'),
                (0x0B, 0x0B),
                (0x0C, 0x0C),
            ],
            true,
        )),

        // \w = [A-Za-z0-9_]
        b'w' => Some((
            vec![(b'0', b'9'), (b'A', b'Z'), (b'a', b'z'), (b'_', b'_')],
            false,
        )),
        b'W' => Some((
            vec![(b'0', b'9'), (b'A', b'Z'), (b'a', b'z'), (b'_', b'_')],
            true,
        )),

        _ => None,
    }
}

// ===== Lexer =====
pub(crate) fn tokenize(pattern: &str) -> Result<Vec<Token>, Error> {
    let bytes = pattern.as_bytes();
    let mut i = 0;
    let n = bytes.len();
    let mut out: Vec<Token> = Vec::new();

    while i < n {
        let c = bytes[i] as char;
        match c {
            '\\' => {
                i += 1;
                if i >= n {
                    return err(ErrorKind::UnexpectedEof, i);
                }
                let esc = bytes[i];

                // 追加: プリセットクラス
                if let Some((ranges, neg)) = predefined_class(esc) {
                    out.push(Token::Class { ranges, neg });
                    i += 1;
                    continue;
                }

                // 制御系のショートエスケープ
                match esc {
                    b't' => out.push(Token::Char(b'\t')),
                    b'n' => out.push(Token::Char(b'\n')),
                    b'r' => out.push(Token::Char(b'\r')),
                    // ここで \. \* \+ \? \| \( \) \[ \] \\ などは
                    // 「その文字をリテラルとして扱う」= Char でOK
                    other => out.push(Token::Char(other)),
                }
                i += 1;
            }
            '.' => {
                out.push(Token::Dot);
                i += 1;
            }
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            '|' => {
                out.push(Token::Alt);
                i += 1;
            }
            '*' => {
                out.push(Token::Star);
                i += 1;
            }
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            '?' => {
                out.push(Token::Qmark);
                i += 1;
            }
            '[' => {
                let (token, j) = parse_class(bytes, i + 1)?; // 既存
                out.push(token);
                i = j;
            }
            _ => {
                out.push(Token::Char(bytes[i]));
                i += 1;
            }
        }
    }
    Ok(out)
}

fn parse_class(bytes: &[u8], mut i: usize) -> Result<(Token, usize), Error> {
    let mut neg = false;
    let mut ranges = Vec::new();

    // 先頭が ^ なら否定クラス
    if i < bytes.len() && bytes[i] == b'^' {
        neg = true;
        i += 1;
    }

    let start = i;
    while i < bytes.len() {
        if bytes[i] == b']' && i > start {
            // クラス終端
            return Ok((Token::Class { ranges, neg }, i + 1));
        }

        let c1 = bytes[i];
        i += 1;

        if i + 1 < bytes.len() && bytes[i] == b'-' && bytes[i + 1] != b']' {
            // 範囲 a-z
            let c2 = bytes[i + 1];
            ranges.push((c1, c2));
            i += 2;
        } else {
            // 単一文字
            ranges.push((c1, c1));
        }
    }

    err(ErrorKind::UnbalancedClass, i)
}
