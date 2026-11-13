//! Recursive-descent parser.
//!
//! The grammar is the subset of spec 0003 implemented by the first vertical
//! slice: one unit, top-level `const`/`var`/`procedure`, a `var` section inside
//! a procedure, `begin`/`end`, `if`/`else`, `while`, assignment, calls and
//! expressions. `function`, records, arrays, streams and handlers are rejected
//! by syntax, which is what "not implemented" should look like.
//!
//! Nodes and sibling lists are allocated in the caller's arena and borrow the
//! token stream; the parser owns nothing.

use core::fmt;

use crate::arena::{Bump, SliceVec};
use crate::ast::*;
use crate::lexer::{Token, TokenKind};
use crate::{diag, out_of_space, Diagnostic, Resource};

struct Parser<'a, 'b> {
    tokens: &'a [Token<'a>],
    pos: usize,
    path: &'a str,
    unit: &'a str,
    bump: &'b Bump<'a>,
}

/// Parses a token stream into a unit.
pub fn parse<'a>(
    tokens: &'a [Token<'a>],
    path: &'a str,
    bump: &Bump<'a>,
) -> Result<Unit<'a>, Diagnostic> {
    let mut parser = Parser {
        tokens,
        pos: 0,
        path,
        unit: "",
        bump,
    };
    parser.unit()
}

impl<'a, 'b> Parser<'a, 'b> {
    fn peek(&self) -> Token<'a> {
        self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn advance(&mut self) -> Token<'a> {
        let token = self.peek();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn error(&self, line: u32, column: u32, message: fmt::Arguments<'_>) -> Diagnostic {
        diag(self.path, self.unit, line, column, message)
    }

    fn here(&self, message: fmt::Arguments<'_>) -> Diagnostic {
        let token = self.peek();
        self.error(token.line, token.column, message)
    }

    fn check(&self, kind: &TokenKind<'_>) -> bool {
        core::mem::discriminant(&self.peek().kind) == core::mem::discriminant(kind)
    }

    fn eat(&mut self, kind: &TokenKind<'_>) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind<'_>, what: &str) -> Result<Token<'a>, Diagnostic> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            let found = self.peek().kind;
            Err(self.here(format_args!("expected {what}, found {found}")))
        }
    }

    fn ident(&mut self, what: &str) -> Result<&'a str, Diagnostic> {
        match self.peek().kind {
            TokenKind::Ident(name) => {
                self.advance();
                Ok(name)
            }
            other => {
                let found = other;
                Err(self.here(format_args!("expected {what}, found {found}")))
            }
        }
    }

    // ---- arena helpers -------------------------------------------------

    fn arena_error(&self, line: u32, column: u32) -> Diagnostic {
        out_of_space(self.path, self.unit, line, column, Resource::Arena)
    }

    fn list<T: Copy>(&self, line: u32, column: u32) -> Result<SliceVec<'a, 'b, T>, Diagnostic> {
        SliceVec::new(self.bump).map_err(|_| self.arena_error(line, column))
    }

    fn push<T: Copy>(
        &self,
        list: &mut SliceVec<'a, 'b, T>,
        item: T,
        line: u32,
        column: u32,
    ) -> Result<(), Diagnostic> {
        list.push(item).map_err(|_| self.arena_error(line, column))
    }

    fn alloc_expr(
        &self,
        kind: ExprKind<'a>,
        line: u32,
        column: u32,
    ) -> Result<&'a Expr<'a>, Diagnostic> {
        self.bump
            .alloc(Expr { kind, line, column })
            .map(|node| &*node)
            .map_err(|_| self.arena_error(line, column))
    }

    fn alloc_stmt(
        &self,
        stmt: Stmt<'a>,
        line: u32,
        column: u32,
    ) -> Result<&'a Stmt<'a>, Diagnostic> {
        self.bump
            .alloc(stmt)
            .map(|node| &*node)
            .map_err(|_| self.arena_error(line, column))
    }

    fn join(&self, parts: &[&'a str], line: u32, column: u32) -> Result<&'a str, Diagnostic> {
        let mut total = parts.len() - 1;
        for part in parts {
            total += part.len();
        }
        let bytes = self
            .bump
            .alloc_slice::<u8>(total, 0)
            .map_err(|_| self.arena_error(line, column))?;
        let mut at = 0;
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                bytes[at] = b'.';
                at += 1;
            }
            bytes[at..at + part.len()].copy_from_slice(part.as_bytes());
            at += part.len();
        }
        core::str::from_utf8(&bytes[..]).map_err(|_| self.arena_error(line, column))
    }

    // ---- declarations --------------------------------------------------

    fn unit(&mut self) -> Result<Unit<'a>, Diagnostic> {
        let start = self.peek();
        self.expect(&TokenKind::Unit, "`unit`")?;
        let name = self.ident("a unit name")?;
        self.unit = name;
        self.expect(&TokenKind::Semi, "`;`")?;

        let mut uses = self.list::<&'a str>(start.line, start.column)?;
        if self.eat(&TokenKind::Uses) {
            loop {
                let used = self.ident("a unit name")?;
                self.push(&mut uses, used, start.line, start.column)?;
                if self.eat(&TokenKind::Comma) {
                    continue;
                }
                break;
            }
            self.expect(&TokenKind::Semi, "`;`")?;
        }

        let mut consts = self.list::<ConstDecl<'a>>(start.line, start.column)?;
        let mut vars = self.list::<VarDecl<'a>>(start.line, start.column)?;
        let mut procs = self.list::<ProcDecl<'a>>(start.line, start.column)?;

        loop {
            let token = self.peek();
            match token.kind {
                TokenKind::Const => self.const_section(&mut consts)?,
                TokenKind::Var => self.var_section(&mut vars)?,
                TokenKind::Procedure => {
                    let proc = self.procedure()?;
                    self.push(&mut procs, proc, token.line, token.column)?;
                }
                TokenKind::Eof => break,
                TokenKind::End => {
                    // A stray `end` most often means a missing `begin`.
                    return Err(self.here(format_args!("unexpected `end`; expected a declaration")));
                }
                other => {
                    return Err(self.here(format_args!(
                        "expected `const`, `var`, `procedure` or end of file, found {other}"
                    )))
                }
            }
        }

        Ok(Unit {
            name,
            uses: uses.into_slice(),
            consts: consts.into_slice(),
            vars: vars.into_slice(),
            procs: procs.into_slice(),
        })
    }

    fn const_section(
        &mut self,
        decls: &mut SliceVec<'a, 'b, ConstDecl<'a>>,
    ) -> Result<(), Diagnostic> {
        let start = self.peek();
        self.expect(&TokenKind::Const, "`const`")?;
        let mut count = 0;
        while matches!(self.peek().kind, TokenKind::Ident(_)) {
            let token = self.peek();
            let name = self.ident("a constant name")?;
            self.expect(&TokenKind::Eq, "`=`")?;
            let value = self.expression()?;
            self.expect(&TokenKind::Semi, "`;`")?;
            let decl = ConstDecl {
                name,
                value,
                line: token.line,
                column: token.column,
            };
            self.push(decls, decl, token.line, token.column)?;
            count += 1;
        }
        if count == 0 {
            return Err(self.error(
                start.line,
                start.column,
                format_args!("`const` must be followed by at least one `Name = value;`"),
            ));
        }
        Ok(())
    }

    fn var_section(&mut self, decls: &mut SliceVec<'a, 'b, VarDecl<'a>>) -> Result<(), Diagnostic> {
        let start = self.peek();
        self.expect(&TokenKind::Var, "`var`")?;
        let mut count = 0;
        while matches!(self.peek().kind, TokenKind::Ident(_)) {
            let token = self.peek();
            let mut names = self.list::<&'a str>(token.line, token.column)?;
            let first = self.ident("a variable name")?;
            self.push(&mut names, first, token.line, token.column)?;
            while self.eat(&TokenKind::Comma) {
                let name = self.ident("a variable name")?;
                self.push(&mut names, name, token.line, token.column)?;
            }
            self.expect(&TokenKind::Colon, "`:`")?;
            let ty = self.type_ref()?;
            self.expect(&TokenKind::Semi, "`;`")?;
            let names_len = names.len();
            for index in 0..names_len {
                let name = names.as_slice()[index];
                let decl = VarDecl {
                    name,
                    ty,
                    line: token.line,
                    column: token.column,
                };
                self.push(decls, decl, token.line, token.column)?;
                count += 1;
            }
        }
        if count == 0 {
            return Err(self.error(
                start.line,
                start.column,
                format_args!("`var` must be followed by at least one `name: type;`"),
            ));
        }
        Ok(())
    }

    fn type_ref(&mut self) -> Result<TypeRef<'a>, Diagnostic> {
        let token = self.peek();
        let name = self.ident("a type name")?;
        let cap = if matches!(name, "string" | "bytes") {
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

    fn procedure(&mut self) -> Result<ProcDecl<'a>, Diagnostic> {
        let token = self.peek();
        self.expect(&TokenKind::Procedure, "`procedure`")?;
        let name = self.ident("a procedure name")?;

        self.expect(&TokenKind::LParen, "`(`")?;
        let mut params = self.list::<ParamDecl<'a>>(token.line, token.column)?;
        if !self.check(&TokenKind::RParen) {
            loop {
                let param_token = self.peek();
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
                let param = ParamDecl {
                    name: param_name,
                    mode,
                    ty,
                    line: param_token.line,
                    column: param_token.column,
                };
                self.push(&mut params, param, param_token.line, param_token.column)?;
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

        let mut locals = self.list::<VarDecl<'a>>(token.line, token.column)?;
        if self.check(&TokenKind::Var) {
            self.var_section(&mut locals)?;
        }

        self.expect(&TokenKind::Begin, "`begin`")?;
        let body = self.block_body()?;
        self.expect(&TokenKind::End, "`end`")?;
        self.expect(&TokenKind::Semi, "`;` after `end`")?;

        Ok(ProcDecl {
            name,
            params: params.into_slice(),
            locals: locals.into_slice(),
            body,
            line: token.line,
            column: token.column,
        })
    }

    fn block_body(&mut self) -> Result<&'a [Stmt<'a>], Diagnostic> {
        let start = self.peek();
        let mut stmts = self.list::<Stmt<'a>>(start.line, start.column)?;
        loop {
            if self.check(&TokenKind::End) || self.check(&TokenKind::Eof) {
                break;
            }
            if self.eat(&TokenKind::Semi) {
                continue;
            }
            let at = self.peek();
            let stmt = self.statement()?;
            self.push(&mut stmts, *stmt, at.line, at.column)?;
        }
        Ok(stmts.into_slice())
    }

    fn statement(&mut self) -> Result<&'a Stmt<'a>, Diagnostic> {
        let token = self.peek();
        match token.kind {
            TokenKind::Begin => {
                self.advance();
                let body = self.block_body()?;
                self.expect(&TokenKind::End, "`end`")?;
                self.alloc_stmt(Stmt::Block(body), token.line, token.column)
            }
            TokenKind::If => {
                self.advance();
                let cond = self.expression()?;
                self.expect(&TokenKind::Then, "`then`")?;
                let then_branch = self.statement()?;
                let else_branch = if self.eat(&TokenKind::Else) {
                    Some(self.statement()?)
                } else {
                    None
                };
                self.alloc_stmt(
                    Stmt::If {
                        cond,
                        then_branch,
                        else_branch,
                    },
                    token.line,
                    token.column,
                )
            }
            TokenKind::While => {
                self.advance();
                let cond = self.expression()?;
                self.expect(&TokenKind::Do, "`do`")?;
                let body = self.statement()?;
                self.alloc_stmt(Stmt::While { cond, body }, token.line, token.column)
            }
            TokenKind::Ident(_) => {
                let (target, line, column) = self.name_path()?;
                if self.eat(&TokenKind::Assign) {
                    if target.contains('.') {
                        return Err(self.error(
                            line,
                            column,
                            format_args!("cannot assign to `{target}`; only variables and parameters are assignable"),
                        ));
                    }
                    let value = self.expression()?;
                    self.alloc_stmt(
                        Stmt::Assign {
                            target,
                            value,
                            line,
                            column,
                        },
                        line,
                        column,
                    )
                } else if self.check(&TokenKind::LParen) {
                    let call = self.call(target, line, column)?;
                    self.alloc_stmt(Stmt::Call(call), line, column)
                } else {
                    let found = self.peek().kind;
                    Err(self.here(format_args!(
                        "expected `:=` or `(` after `{target}`, found {found}"
                    )))
                }
            }
            other => Err(self.here(format_args!("expected a statement, found {other}"))),
        }
    }

    fn name_path(&mut self) -> Result<(&'a str, u32, u32), Diagnostic> {
        let token = self.peek();
        let first = self.ident("an identifier")?;
        if !self.check(&TokenKind::Dot) {
            return Ok((first, token.line, token.column));
        }
        let mut parts = self.list::<&'a str>(token.line, token.column)?;
        self.push(&mut parts, first, token.line, token.column)?;
        while self.eat(&TokenKind::Dot) {
            let part = self.ident("a name after `.`")?;
            self.push(&mut parts, part, token.line, token.column)?;
        }
        let name = self.join(parts.as_slice(), token.line, token.column)?;
        Ok((name, token.line, token.column))
    }

    fn call(&mut self, target: &'a str, line: u32, column: u32) -> Result<Call<'a>, Diagnostic> {
        self.expect(&TokenKind::LParen, "`(`")?;
        let mut args = self.list::<&'a Expr<'a>>(line, column)?;
        if !self.check(&TokenKind::RParen) {
            loop {
                let arg = self.expression()?;
                self.push(&mut args, arg, line, column)?;
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
            args: args.into_slice(),
            line,
            column,
        })
    }

    // ---- expressions ---------------------------------------------------

    fn expression(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
        self.parse_or()
    }

    fn binary(
        &self,
        left: &'a Expr<'a>,
        op: BinOp,
        right: &'a Expr<'a>,
    ) -> Result<&'a Expr<'a>, Diagnostic> {
        self.alloc_expr(ExprKind::Binary(op, left, right), left.line, left.column)
    }

    fn parse_or(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
        let mut left = self.parse_and()?;
        while self.eat(&TokenKind::Or) {
            let right = self.parse_and()?;
            left = self.binary(left, BinOp::Or, right)?;
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
        let mut left = self.parse_equality()?;
        while self.eat(&TokenKind::And) {
            let right = self.parse_equality()?;
            left = self.binary(left, BinOp::And, right)?;
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
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
                    left = self.binary(left, op, right)?;
                }
                None => return Ok(left),
            }
        }
    }

    fn parse_relational(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
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
                    left = self.binary(left, op, right)?;
                }
                None => return Ok(left),
            }
        }
    }

    fn parse_additive(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
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
                    left = self.binary(left, op, right)?;
                }
                None => return Ok(left),
            }
        }
    }

    fn parse_multiplicative(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
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
                    left = self.binary(left, op, right)?;
                }
                None => return Ok(left),
            }
        }
    }

    fn parse_unary(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
        let token = self.peek();
        if self.eat(&TokenKind::Minus) {
            let operand = self.parse_unary()?;
            return self.alloc_expr(
                ExprKind::Unary(UnOp::Neg, operand),
                token.line,
                token.column,
            );
        }
        if self.eat(&TokenKind::Not) {
            let operand = self.parse_unary()?;
            return self.alloc_expr(
                ExprKind::Unary(UnOp::Not, operand),
                token.line,
                token.column,
            );
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
        let token = self.peek();
        match token.kind {
            TokenKind::Int(value) => {
                self.advance();
                self.alloc_expr(ExprKind::Int(value), token.line, token.column)
            }
            TokenKind::Float(value) => {
                self.advance();
                self.alloc_expr(ExprKind::Float(value), token.line, token.column)
            }
            TokenKind::Str(bytes) => {
                self.advance();
                self.alloc_expr(ExprKind::Str(bytes), token.line, token.column)
            }
            TokenKind::True | TokenKind::False => {
                let value = matches!(token.kind, TokenKind::True);
                self.advance();
                self.alloc_expr(ExprKind::Bool(value), token.line, token.column)
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
                    let is_cast =
                        !name.contains('.') && matches!(name, "int" | "uint" | "byte" | "f64");
                    if is_cast {
                        self.advance();
                        let inner = self.expression()?;
                        self.expect(&TokenKind::RParen, "`)`")?;
                        return self.alloc_expr(ExprKind::Cast(name, inner), line, column);
                    }
                    let call = self.call(name, line, column)?;
                    self.alloc_expr(ExprKind::Call(call), line, column)
                } else if name.contains('.') {
                    Err(self.here(format_args!("expected `(` after `{name}`")))
                } else {
                    self.alloc_expr(ExprKind::Ident(name), line, column)
                }
            }
            other => Err(self.here(format_args!("expected an expression, found {other}"))),
        }
    }
}
