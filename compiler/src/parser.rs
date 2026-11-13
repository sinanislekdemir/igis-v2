//! Recursive-descent parser.
//!
//! The grammar is the subset of spec 0003 implemented by the first vertical
//! slice: one unit, top-level `const`/`var`/`procedure`, a `var` section inside
//! a procedure, `begin`/`end`, `if`/`else`, `while`, assignment, calls and
//! expressions. `function`, records, arrays, streams and handlers are rejected
//! by syntax, which is what "not implemented" should look like.

use crate::ast::*;
use crate::diag;
use crate::lexer::{Token, TokenKind};
use crate::Diagnostic;

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    path: &'a str,
    unit: String,
}

/// Parses a token stream into a unit.
pub fn parse(tokens: &[Token], path: &str) -> Result<Unit, Diagnostic> {
    let mut parser = Parser {
        tokens,
        pos: 0,
        path,
        unit: String::new(),
    };
    parser.unit()
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn advance(&mut self) -> Token {
        let token = self.peek().clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn error(&self, line: u32, column: u32, message: impl Into<String>) -> Diagnostic {
        diag(self.path, &self.unit, line, column, message)
    }

    fn here(&self, message: impl Into<String>) -> Diagnostic {
        let token = self.peek();
        self.error(token.line, token.column, message)
    }

    fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(&self.peek().kind) == std::mem::discriminant(kind)
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind, what: &str) -> Result<Token, Diagnostic> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            Err(self.here(format!(
                "expected {what}, found {}",
                self.peek().kind.describe()
            )))
        }
    }

    fn ident(&mut self, what: &str) -> Result<String, Diagnostic> {
        match &self.peek().kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(name)
            }
            other => Err(self.here(format!("expected {what}, found {}", other.describe()))),
        }
    }

    fn unit(&mut self) -> Result<Unit, Diagnostic> {
        self.expect(&TokenKind::Unit, "`unit`")?;
        let name = self.ident("a unit name")?;
        self.unit = name.clone();
        self.expect(&TokenKind::Semi, "`;`")?;

        let mut uses = Vec::new();
        if self.eat(&TokenKind::Uses) {
            loop {
                uses.push(self.ident("a unit name")?);
                if self.eat(&TokenKind::Comma) {
                    continue;
                }
                break;
            }
            self.expect(&TokenKind::Semi, "`;`")?;
        }

        let mut unit = Unit {
            name,
            uses,
            consts: Vec::new(),
            vars: Vec::new(),
            procs: Vec::new(),
        };

        loop {
            match &self.peek().kind {
                TokenKind::Const => unit.consts.extend(self.const_section()?),
                TokenKind::Var => unit.vars.extend(self.var_section()?),
                TokenKind::Procedure => unit.procs.push(self.procedure()?),
                TokenKind::Eof => break,
                TokenKind::End => {
                    // A stray `end` most often means a missing `begin`.
                    return Err(self.here("unexpected `end`; expected a declaration"));
                }
                other => {
                    return Err(self.here(format!(
                        "expected `const`, `var`, `procedure` or end of file, found {}",
                        other.describe()
                    )))
                }
            }
        }

        Ok(unit)
    }

    fn const_section(&mut self) -> Result<Vec<ConstDecl>, Diagnostic> {
        let start = self.peek().clone();
        self.expect(&TokenKind::Const, "`const`")?;
        let mut decls = Vec::new();
        while matches!(self.peek().kind, TokenKind::Ident(_)) {
            let token = self.peek().clone();
            let name = self.ident("a constant name")?;
            self.expect(&TokenKind::Eq, "`=`")?;
            let value = self.expression()?;
            self.expect(&TokenKind::Semi, "`;`")?;
            decls.push(ConstDecl {
                name,
                value,
                line: token.line,
                column: token.column,
            });
        }
        if decls.is_empty() {
            return Err(self.error(
                start.line,
                start.column,
                "`const` must be followed by at least one `Name = value;`",
            ));
        }
        Ok(decls)
    }

    fn var_section(&mut self) -> Result<Vec<VarDecl>, Diagnostic> {
        let start = self.peek().clone();
        self.expect(&TokenKind::Var, "`var`")?;
        let mut decls = Vec::new();
        while matches!(self.peek().kind, TokenKind::Ident(_)) {
            let token = self.peek().clone();
            let mut names = vec![self.ident("a variable name")?];
            while self.eat(&TokenKind::Comma) {
                names.push(self.ident("a variable name")?);
            }
            self.expect(&TokenKind::Colon, "`:`")?;
            let ty = self.type_ref()?;
            self.expect(&TokenKind::Semi, "`;`")?;
            for name in names {
                decls.push(VarDecl {
                    name,
                    ty: ty.clone(),
                    line: token.line,
                    column: token.column,
                });
            }
        }
        if decls.is_empty() {
            return Err(self.error(
                start.line,
                start.column,
                "`var` must be followed by at least one `name: type;`",
            ));
        }
        Ok(decls)
    }

    fn type_ref(&mut self) -> Result<TypeRef, Diagnostic> {
        let token = self.peek().clone();
        let name = self.ident("a type name")?;
        let cap = if matches!(name.as_str(), "string" | "bytes") {
            self.expect(&TokenKind::LBracket, "`[` after `string`/`bytes`")?;
            let cap = self.expression()?;
            self.expect(&TokenKind::RBracket, "`]`")?;
            Some(cap)
        } else {
            None
        };
        Ok(TypeRef {
            name,
            cap,
            line: token.line,
            column: token.column,
        })
    }

    fn procedure(&mut self) -> Result<ProcDecl, Diagnostic> {
        let token = self.peek().clone();
        self.expect(&TokenKind::Procedure, "`procedure`")?;
        let name = self.ident("a procedure name")?;

        self.expect(&TokenKind::LParen, "`(`")?;
        let mut params = Vec::new();
        if !self.check(&TokenKind::RParen) {
            loop {
                let param_token = self.peek().clone();
                let param_name = self.ident("a parameter name")?;
                self.expect(&TokenKind::Colon, "`:`")?;
                // The mode is written after the colon, as in
                // `main(args: const string[1024])`.
                let mode = if self.eat(&TokenKind::Const) {
                    ParamMode::Const
                } else if self.eat(&TokenKind::Var) {
                    ParamMode::Var
                } else {
                    ParamMode::Value
                };
                let ty = self.type_ref()?;
                params.push(ParamDecl {
                    name: param_name,
                    mode,
                    ty,
                    line: param_token.line,
                    column: param_token.column,
                });
                if self.eat(&TokenKind::Semi) || self.eat(&TokenKind::Comma) {
                    if self.check(&TokenKind::RParen) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        self.expect(&TokenKind::Semi, "`;`")?;

        let mut locals = Vec::new();
        if self.check(&TokenKind::Var) {
            locals = self.var_section()?;
        }

        self.expect(&TokenKind::Begin, "`begin`")?;
        let body = self.block_body()?;
        self.expect(&TokenKind::End, "`end`")?;
        self.expect(&TokenKind::Semi, "`;` after `end`")?;

        Ok(ProcDecl {
            name,
            params,
            locals,
            body,
            line: token.line,
            column: token.column,
        })
    }

    fn block_body(&mut self) -> Result<Vec<Stmt>, Diagnostic> {
        let mut stmts = Vec::new();
        loop {
            if self.check(&TokenKind::End) || self.check(&TokenKind::Eof) {
                break;
            }
            if self.eat(&TokenKind::Semi) {
                continue;
            }
            stmts.push(self.statement()?);
        }
        Ok(stmts)
    }

    fn statement(&mut self) -> Result<Stmt, Diagnostic> {
        match &self.peek().kind {
            TokenKind::Begin => {
                self.advance();
                let body = self.block_body()?;
                self.expect(&TokenKind::End, "`end`")?;
                Ok(Stmt::Block(body))
            }
            TokenKind::If => {
                self.advance();
                let cond = self.expression()?;
                self.expect(&TokenKind::Then, "`then`")?;
                let then_branch = Box::new(self.statement()?);
                let else_branch = if self.eat(&TokenKind::Else) {
                    Some(Box::new(self.statement()?))
                } else {
                    None
                };
                Ok(Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                })
            }
            TokenKind::While => {
                self.advance();
                let cond = self.expression()?;
                self.expect(&TokenKind::Do, "`do`")?;
                let body = Box::new(self.statement()?);
                Ok(Stmt::While { cond, body })
            }
            TokenKind::Ident(_) => {
                let (target, line, column) = self.name_path()?;
                if self.eat(&TokenKind::Assign) {
                    if target.contains('.') {
                        return Err(self.error(
                            line,
                            column,
                            format!("cannot assign to `{target}`; only variables and parameters are assignable"),
                        ));
                    }
                    let value = self.expression()?;
                    Ok(Stmt::Assign {
                        target,
                        value,
                        line,
                        column,
                    })
                } else if self.check(&TokenKind::LParen) {
                    let call = self.call(target, line, column)?;
                    Ok(Stmt::Call(call))
                } else {
                    Err(self.here(format!(
                        "expected `:=` or `(` after `{target}`, found {}",
                        self.peek().kind.describe()
                    )))
                }
            }
            other => Err(self.here(format!("expected a statement, found {}", other.describe()))),
        }
    }

    fn name_path(&mut self) -> Result<(String, u32, u32), Diagnostic> {
        let token = self.peek().clone();
        let mut name = self.ident("an identifier")?;
        while self.eat(&TokenKind::Dot) {
            let part = self.ident("a name after `.`")?;
            name.push('.');
            name.push_str(&part);
        }
        Ok((name, token.line, token.column))
    }

    fn call(&mut self, target: String, line: u32, column: u32) -> Result<Call, Diagnostic> {
        self.expect(&TokenKind::LParen, "`(`")?;
        let mut args = Vec::new();
        if !self.check(&TokenKind::RParen) {
            loop {
                args.push(self.expression()?);
                if self.eat(&TokenKind::Comma) {
                    if self.check(&TokenKind::RParen) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        Ok(Call {
            target,
            args,
            line,
            column,
        })
    }

    fn expression(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_or()
    }

    fn binary(left: Expr, op: BinOp, right: Expr) -> Expr {
        let line = left.line;
        let column = left.column;
        Expr {
            kind: ExprKind::Binary(op, Box::new(left), Box::new(right)),
            line,
            column,
        }
    }

    fn parse_or(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_and()?;
        while self.eat(&TokenKind::Or) {
            let right = self.parse_and()?;
            left = Self::binary(left, BinOp::Or, right);
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_equality()?;
        while self.eat(&TokenKind::And) {
            let right = self.parse_equality()?;
            left = Self::binary(left, BinOp::And, right);
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_relational()?;
        loop {
            let op = if self.eat(&TokenKind::Eq) {
                Some(BinOp::Eq)
            } else if self.eat(&TokenKind::Ne) {
                Some(BinOp::Ne)
            } else {
                None
            };
            match op {
                Some(op) => {
                    let right = self.parse_relational()?;
                    left = Self::binary(left, op, right);
                }
                None => return Ok(left),
            }
        }
    }

    fn parse_relational(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_additive()?;
        loop {
            let op = if self.eat(&TokenKind::Lt) {
                Some(BinOp::Lt)
            } else if self.eat(&TokenKind::Le) {
                Some(BinOp::Le)
            } else if self.eat(&TokenKind::Gt) {
                Some(BinOp::Gt)
            } else if self.eat(&TokenKind::Ge) {
                Some(BinOp::Ge)
            } else {
                None
            };
            match op {
                Some(op) => {
                    let right = self.parse_additive()?;
                    left = Self::binary(left, op, right);
                }
                None => return Ok(left),
            }
        }
    }

    fn parse_additive(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = if self.eat(&TokenKind::Plus) {
                Some(BinOp::Add)
            } else if self.eat(&TokenKind::Minus) {
                Some(BinOp::Sub)
            } else {
                None
            };
            match op {
                Some(op) => {
                    let right = self.parse_multiplicative()?;
                    left = Self::binary(left, op, right);
                }
                None => return Ok(left),
            }
        }
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, Diagnostic> {
        let mut left = self.parse_unary()?;
        loop {
            let op = if self.eat(&TokenKind::Star) {
                Some(BinOp::Mul)
            } else if self.eat(&TokenKind::Slash) {
                Some(BinOp::Slash)
            } else if self.eat(&TokenKind::Div) {
                Some(BinOp::Div)
            } else if self.eat(&TokenKind::Mod) {
                Some(BinOp::Mod)
            } else {
                None
            };
            match op {
                Some(op) => {
                    let right = self.parse_unary()?;
                    left = Self::binary(left, op, right);
                }
                None => return Ok(left),
            }
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, Diagnostic> {
        if self.eat(&TokenKind::Minus) {
            let token = self.tokens[self.pos - 1].clone();
            let operand = self.parse_unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Neg, Box::new(operand)),
                line: token.line,
                column: token.column,
            });
        }
        if self.eat(&TokenKind::Not) {
            let token = self.tokens[self.pos - 1].clone();
            let operand = self.parse_unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Not, Box::new(operand)),
                line: token.line,
                column: token.column,
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, Diagnostic> {
        let token = self.peek().clone();
        match &token.kind {
            TokenKind::Int(value) => {
                let value = *value;
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Int(value),
                    line: token.line,
                    column: token.column,
                })
            }
            TokenKind::Float(value) => {
                let value = *value;
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Float(value),
                    line: token.line,
                    column: token.column,
                })
            }
            TokenKind::Str(bytes) => {
                let bytes = bytes.clone();
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Str(bytes),
                    line: token.line,
                    column: token.column,
                })
            }
            TokenKind::True | TokenKind::False => {
                let value = token.kind == TokenKind::True;
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Bool(value),
                    line: token.line,
                    column: token.column,
                })
            }
            TokenKind::LParen => {
                self.advance();
                let inner = self.expression()?;
                self.expect(&TokenKind::RParen, "`)`")?;
                Ok(inner)
            }
            TokenKind::Ident(_) => {
                let (name, line, column) = self.name_path()?;
                if self.check(&TokenKind::LParen) {
                    let is_cast = !name.contains('.')
                        && matches!(name.as_str(), "int" | "uint" | "byte" | "f64");
                    if is_cast {
                        self.advance();
                        let inner = self.expression()?;
                        self.expect(&TokenKind::RParen, "`)`")?;
                        return Ok(Expr {
                            kind: ExprKind::Cast(name, Box::new(inner)),
                            line,
                            column,
                        });
                    }
                    let call = self.call(name, line, column)?;
                    Ok(Expr {
                        kind: ExprKind::Call(call),
                        line,
                        column,
                    })
                } else if name.contains('.') {
                    Err(self.here(format!("expected `(` after `{name}`")))
                } else {
                    Ok(Expr {
                        kind: ExprKind::Ident(name),
                        line,
                        column,
                    })
                }
            }
            other => Err(self.here(format!(
                "expected an expression, found {}",
                other.describe()
            ))),
        }
    }
}
