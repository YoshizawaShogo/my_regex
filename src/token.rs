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

    CapStart(usize),
    CapEnd(usize),
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

#[cfg(test)]
mod tests {
    use super::*;

    // ちょい便利: クラスのrangeを短く書く
    fn r(a: u8, b: u8) -> (u8, u8) {
        (a, b)
    }

    #[test]
    fn literal_and_metachars() {
        let got = tokenize("ab.c|()").unwrap();
        assert_eq!(
            got,
            vec![
                Token::Char(b'a'),
                Token::Char(b'b'),
                Token::Dot,
                Token::Char(b'c'),
                Token::Alt,
                Token::LParen,
                Token::RParen,
            ]
        );
    }

    #[test]
    fn quantifiers() {
        let got = tokenize("a*+?").unwrap();
        assert_eq!(
            got,
            vec![Token::Char(b'a'), Token::Star, Token::Plus, Token::Qmark,]
        );
    }

    #[test]
    fn simple_escapes_and_literal_escapes() {
        let got = tokenize(r"\t\n\r\.\*\+\?\|\(\)\[\]\\X").unwrap();
        assert_eq!(
            got,
            vec![
                Token::Char(b'\t'),
                Token::Char(b'\n'),
                Token::Char(b'\r'),
                Token::Char(b'.'),
                Token::Char(b'*'),
                Token::Char(b'+'),
                Token::Char(b'?'),
                Token::Char(b'|'),
                Token::Char(b'('),
                Token::Char(b')'),
                Token::Char(b'['),
                Token::Char(b']'),
                Token::Char(b'\\'),
                Token::Char(b'X'),
            ]
        );
    }

    #[test]
    fn presets_digit_space_word_positive() {
        let got = tokenize(r"\d\s\w").unwrap();
        assert_eq!(
            got,
            vec![
                Token::Class {
                    ranges: vec![r(b'0', b'9')],
                    neg: false
                },
                Token::Class {
                    ranges: vec![
                        r(b' ', b' '),
                        r(b'\t', b'\t'),
                        r(b'\n', b'\n'),
                        r(b'\r', b'\r'),
                        r(0x0B, 0x0B),
                        r(0x0C, 0x0C)
                    ],
                    neg: false
                },
                Token::Class {
                    ranges: vec![r(b'0', b'9'), r(b'A', b'Z'), r(b'a', b'z'), r(b'_', b'_')],
                    neg: false
                },
            ]
        );
    }

    #[test]
    fn presets_digit_space_word_negative() {
        let got = tokenize(r"\D\S\W").unwrap();
        assert_eq!(
            got,
            vec![
                Token::Class {
                    ranges: vec![r(b'0', b'9')],
                    neg: true
                },
                Token::Class {
                    ranges: vec![
                        r(b' ', b' '),
                        r(b'\t', b'\t'),
                        r(b'\n', b'\n'),
                        r(b'\r', b'\r'),
                        r(0x0B, 0x0B),
                        r(0x0C, 0x0C)
                    ],
                    neg: true
                },
                Token::Class {
                    ranges: vec![r(b'0', b'9'), r(b'A', b'Z'), r(b'a', b'z'), r(b'_', b'_')],
                    neg: true
                },
            ]
        );
    }

    #[test]
    fn char_class_singletons() {
        let got = tokenize("[abc]").unwrap();
        assert_eq!(
            got,
            vec![Token::Class {
                ranges: vec![r(b'a', b'a'), r(b'b', b'b'), r(b'c', b'c')],
                neg: false
            }]
        );
    }

    #[test]
    fn char_class_ranges_and_singletons_mixed() {
        let got = tokenize("[a-cx-z0-9_]").unwrap();
        assert_eq!(
            got,
            vec![Token::Class {
                ranges: vec![r(b'a', b'c'), r(b'x', b'z'), r(b'0', b'9'), r(b'_', b'_'),],
                neg: false
            }]
        );
    }

    #[test]
    fn negated_char_class() {
        let got = tokenize("[^a-z]").unwrap();
        assert_eq!(
            got,
            vec![Token::Class {
                ranges: vec![r(b'a', b'z')],
                neg: true
            }]
        );
    }

    #[test]
    fn dot_and_alt_with_groups_and_class() {
        let got = tokenize("(ab|c.)[0-9]").unwrap();
        assert_eq!(
            got,
            vec![
                Token::LParen,
                Token::Char(b'a'),
                Token::Char(b'b'),
                Token::Alt,
                Token::Char(b'c'),
                Token::Dot,
                Token::RParen,
                Token::Class {
                    ranges: vec![r(b'0', b'9')],
                    neg: false
                },
            ]
        );
    }

    #[test]
    fn trailing_backslash_is_error() {
        let err = tokenize("\\").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedEof));
    }

    #[test]
    fn unbalanced_class_is_error() {
        let err = tokenize("[abc").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnbalancedClass));
    }

    // 端ケース: [a-] と [-a] の扱い
    // 実装は「'-' の直後が ']' でなければ範囲扱い」なので、
    // [-a] -> '-' 単体 + 'a' 単体, [a-] -> 'a' 単体 + '-' 単体 になることを確認
    #[test]
    fn class_dash_edge_cases() {
        let got1 = tokenize("[-a]").unwrap();
        assert_eq!(
            got1,
            vec![Token::Class {
                ranges: vec![r(b'-', b'-'), r(b'a', b'a')],
                neg: false
            }]
        );
        let got2 = tokenize("[a-]").unwrap();
        assert_eq!(
            got2,
            vec![Token::Class {
                ranges: vec![r(b'a', b'a'), r(b'-', b'-')],
                neg: false
            }]
        );
    }

    // プリセットとクラスの混在（トークナイザ段階では分割トークンの並びになる）
    #[test]
    fn presets_mix_with_literals_and_ops() {
        let got = tokenize(r"\w+\s*\d").unwrap();
        assert_eq!(
            got,
            vec![
                Token::Class {
                    ranges: vec![r(b'0', b'9'), r(b'A', b'Z'), r(b'a', b'z'), r(b'_', b'_')],
                    neg: false
                },
                Token::Plus,
                Token::Class {
                    ranges: vec![
                        r(b' ', b' '),
                        r(b'\t', b'\t'),
                        r(b'\n', b'\n'),
                        r(b'\r', b'\r'),
                        r(0x0B, 0x0B),
                        r(0x0C, 0x0C)
                    ],
                    neg: false
                },
                Token::Star,
                Token::Class {
                    ranges: vec![r(b'0', b'9')],
                    neg: false
                },
            ]
        );
    }
}
