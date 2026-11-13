//! Lexer for the igis-v2 application language.
//!
//! Source is UTF-8 text, but the language's identifiers and keywords are ASCII.
//! Columns count bytes and start at 1; a tab advances the column by one. The
//! block-comment form is `{ ... }`; the line form is `// ...`.

use crate::{diag, Diagnostic};

/// One lexical token.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// Token class and payload.
    pub kind: TokenKind,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// Token classes.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// Identifier.
    Ident(String),
    /// Signed integer literal.
    Int(i64),
    /// Floating-point literal.
    Float(f64),
    /// String literal as raw bytes (escapes already decoded).
    Str(Vec<u8>),
    /// `unit`
    Unit,
    /// `uses`
    Uses,
    /// `const`
    Const,
    /// `var`
    Var,
    /// `procedure`
    Procedure,
    /// `begin`
    Begin,
    /// `end`
    End,
    /// `if`
    If,
    /// `then`
    Then,
    /// `else`
    Else,
    /// `while`
    While,
    /// `do`
    Do,
    /// `and`
    And,
    /// `or`
    Or,
    /// `not`
    Not,
    /// `div`
    Div,
    /// `mod`
    Mod,
    /// `true`
    True,
    /// `false`
    False,
    /// `:=`
    Assign,
    /// `=`
    Eq,
    /// `<>`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `;`
    Semi,
    /// `:`
    Colon,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// End of input.
    Eof,
}

impl TokenKind {
    /// Human-readable description used in syntax errors.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Ident(name) => format!("identifier `{name}`"),
            TokenKind::Int(value) => format!("integer `{value}`"),
            TokenKind::Float(value) => format!("float `{value}`"),
            TokenKind::Str(_) => "string literal".to_string(),
            TokenKind::Eof => "end of file".to_string(),
            other => format!("`{}`", other.text()),
        }
    }

    fn text(&self) -> &'static str {
        match self {
            TokenKind::Unit => "unit",
            TokenKind::Uses => "uses",
            TokenKind::Const => "const",
            TokenKind::Var => "var",
            TokenKind::Procedure => "procedure",
            TokenKind::Begin => "begin",
            TokenKind::End => "end",
            TokenKind::If => "if",
            TokenKind::Then => "then",
            TokenKind::Else => "else",
            TokenKind::While => "while",
            TokenKind::Do => "do",
            TokenKind::And => "and",
            TokenKind::Or => "or",
            TokenKind::Not => "not",
            TokenKind::Div => "div",
            TokenKind::Mod => "mod",
            TokenKind::True => "true",
            TokenKind::False => "false",
            TokenKind::Assign => ":=",
            TokenKind::Eq => "=",
            TokenKind::Ne => "<>",
            TokenKind::Lt => "<",
            TokenKind::Le => "<=",
            TokenKind::Gt => ">",
            TokenKind::Ge => ">=",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::Semi => ";",
            TokenKind::Colon => ":",
            TokenKind::Comma => ",",
            TokenKind::Dot => ".",
            TokenKind::Ident(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Str(_)
            | TokenKind::Eof => "",
        }
    }
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
    column: u32,
    path: &'a str,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a [u8], path: &'a str) -> Self {
        Lexer {
            src,
            pos: 0,
            line: 1,
            column: 1,
            path,
        }
    }

    fn error(&self, line: u32, column: u32, message: impl Into<String>) -> Diagnostic {
        diag(self.path, "", line, column, message)
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.src.get(self.pos + offset).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        if byte == b'\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(byte)
    }

    fn skip_trivia(&mut self) -> Result<(), Diagnostic> {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') => {
                    self.bump();
                }
                Some(b'{') => {
                    let (line, column) = (self.line, self.column);
                    self.bump();
                    loop {
                        match self.bump() {
                            Some(b'}') => break,
                            Some(_) => {}
                            None => {
                                return Err(self.error(line, column, "unterminated block comment"));
                            }
                        }
                    }
                }
                Some(b'/') if self.peek_at(1) == Some(b'/') => {
                    while let Some(byte) = self.bump() {
                        if byte == b'\n' {
                            break;
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn next(&mut self) -> Result<Token, Diagnostic> {
        self.skip_trivia()?;
        let (line, column) = (self.line, self.column);
        let byte = match self.bump() {
            Some(byte) => byte,
            None => {
                return Ok(Token {
                    kind: TokenKind::Eof,
                    line,
                    column,
                })
            }
        };

        let kind = match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.ident(byte),
            b'0'..=b'9' => self.number(byte, line, column)?,
            b'"' => TokenKind::Str(self.string(line, column)?),
            b':' => {
                if self.peek() == Some(b'=') {
                    self.bump();
                    TokenKind::Assign
                } else {
                    TokenKind::Colon
                }
            }
            b'<' => match self.peek() {
                Some(b'>') => {
                    self.bump();
                    TokenKind::Ne
                }
                Some(b'=') => {
                    self.bump();
                    TokenKind::Le
                }
                _ => TokenKind::Lt,
            },
            b'>' => {
                if self.peek() == Some(b'=') {
                    self.bump();
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            b'=' => TokenKind::Eq,
            b'+' => TokenKind::Plus,
            b'-' => TokenKind::Minus,
            b'*' => TokenKind::Star,
            b'/' => TokenKind::Slash,
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            b';' => TokenKind::Semi,
            b',' => TokenKind::Comma,
            b'.' => TokenKind::Dot,
            other => {
                return Err(self.error(
                    line,
                    column,
                    format!("unexpected character `{}`", other as char),
                ))
            }
        };

        Ok(Token { kind, line, column })
    }

    fn ident(&mut self, first: u8) -> TokenKind {
        let mut text = vec![first];
        while let Some(byte) = self.peek() {
            if byte.is_ascii_alphanumeric() || byte == b'_' {
                text.push(byte);
                self.bump();
            } else {
                break;
            }
        }
        // The byte vector is ASCII by construction.
        let text = String::from_utf8(text).expect("identifier is ASCII");
        match text.as_str() {
            "unit" => TokenKind::Unit,
            "uses" => TokenKind::Uses,
            "const" => TokenKind::Const,
            "var" => TokenKind::Var,
            "procedure" => TokenKind::Procedure,
            "begin" => TokenKind::Begin,
            "end" => TokenKind::End,
            "if" => TokenKind::If,
            "then" => TokenKind::Then,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "do" => TokenKind::Do,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "not" => TokenKind::Not,
            "div" => TokenKind::Div,
            "mod" => TokenKind::Mod,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            _ => TokenKind::Ident(text),
        }
    }

    fn number(&mut self, first: u8, line: u32, column: u32) -> Result<TokenKind, Diagnostic> {
        let start = self.pos - 1;

        if first == b'0' && matches!(self.peek(), Some(b'x') | Some(b'X')) {
            self.bump();
            let digits_start = self.pos;
            while let Some(byte) = self.peek() {
                if byte.is_ascii_hexdigit() {
                    self.bump();
                } else {
                    break;
                }
            }
            if self.pos == digits_start {
                return Err(self.error(line, column, "hexadecimal literal has no digits"));
            }
            let text = std::str::from_utf8(&self.src[digits_start..self.pos])
                .map_err(|_| self.error(line, column, "invalid hexadecimal literal"))?;
            return i64::from_str_radix(text, 16)
                .map(TokenKind::Int)
                .map_err(|_| self.error(line, column, "integer literal does not fit in 64 bits"));
        }

        let _ = first;
        let mut is_float = false;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.bump();
        }
        if self.peek() == Some(b'.') && matches!(self.peek_at(1), Some(b'0'..=b'9')) {
            is_float = true;
            self.bump();
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            self.bump();
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.bump();
            }
            let digits_start = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
            if self.pos == digits_start {
                return Err(self.error(line, column, "float exponent has no digits"));
            }
        }

        let text = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|_| self.error(line, column, "invalid number literal"))?;
        if is_float {
            text.parse::<f64>()
                .map(TokenKind::Float)
                .map_err(|_| self.error(line, column, "invalid float literal"))
        } else {
            text.parse::<i64>()
                .map(TokenKind::Int)
                .map_err(|_| self.error(line, column, "integer literal does not fit in 64 bits"))
        }
    }

    fn string(&mut self, line: u32, column: u32) -> Result<Vec<u8>, Diagnostic> {
        let mut out = Vec::new();
        loop {
            let byte = match self.bump() {
                Some(byte) => byte,
                None => return Err(self.error(line, column, "unterminated string literal")),
            };
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let escape = match self.bump() {
                        Some(escape) => escape,
                        None => {
                            return Err(self.error(line, column, "unterminated string literal"))
                        }
                    };
                    match escape {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'\\' => out.push(b'\\'),
                        b'"' => out.push(b'"'),
                        b'x' => {
                            let hi = self.hex_digit().ok_or_else(|| {
                                self.error(line, column, "`\\x` needs two hexadecimal digits")
                            })?;
                            let lo = self.hex_digit().ok_or_else(|| {
                                self.error(line, column, "`\\x` needs two hexadecimal digits")
                            })?;
                            out.push(hi * 16 + lo);
                        }
                        other => {
                            return Err(self.error(
                                line,
                                column,
                                format!("unknown string escape `\\{}`", other as char),
                            ))
                        }
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn hex_digit(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        let value = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        self.bump();
        Some(value)
    }
}

/// Tokenises one unit.
pub fn lex(src: &[u8], path: &str) -> Result<Vec<Token>, Diagnostic> {
    let mut lexer = Lexer::new(src, path);
    let mut tokens = Vec::new();
    loop {
        let token = lexer.next()?;
        let eof = token.kind == TokenKind::Eof;
        tokens.push(token);
        if eof {
            return Ok(tokens);
        }
    }
}
