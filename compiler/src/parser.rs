//! Recursive-descent parser.
//!
//! The grammar is the subset of spec 0003 implemented by the first vertical
//! slice: `unit`/`uses`, top-level `const`/`var`/`procedure`/`function`, a `var`
//! section inside a subprogram, `begin`/`end`, `if`/`else`, `while`, `case`,
//! assignment, calls, `return` and expressions. Records, enums, sum types,
//! fixed-size arrays, rings, maps, stream endpoint declarations and functions
//! are implemented; handlers are rejected by syntax, which is what "not
//! implemented" should look like.
//!
//! Nodes and sibling lists are allocated in the caller's arena and borrow the
//! token stream; the parser owns nothing.

use core::fmt;

use crate::arena::{Bump, SliceVec};
use crate::ast::*;
use crate::lexer::{Token, TokenKind};
use crate::{diag, out_of_space, Diagnostic, Resource, MAX_NESTING};

struct Parser<'a, 'b> {
    tokens: &'a [Token<'a>],
    pos: usize,
    path: &'a str,
    unit: &'a str,
    bump: &'b Bump<'a>,
    /// Every type name declared in a `type` section anywhere in the unit, plus
    /// the predeclared `Status`, `FaultCode`, `Diagnostic` and `ExitReason`.
    /// Collected before parsing so a cast in a procedure that precedes its
    /// `type` section still parses as a cast.
    type_names: SliceVec<'a, 'b, &'a str>,
    /// Nesting levels currently on the recursive-descent stack. Per
    /// compilation, incremented by `expression`, `statement` and recursive
    /// unary operators, and always decremented before returning.
    nesting: u32,
}

/// Parses a token stream into a unit.
pub fn parse<'a>(
    tokens: &'a [Token<'a>],
    path: &'a str,
    bump: &Bump<'a>,
) -> Result<Unit<'a>, Diagnostic> {
    // Collect declared type names before parsing: a procedure may precede the
    // `type` section that defines a name it casts to. `Status` is predeclared
    // (spec 0003, "Errors") and is always spellable as a cast.
    let mut type_names =
        SliceVec::new(bump).map_err(|_| crate::out_of_space(path, "", 1, 1, Resource::Arena))?;
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != TokenKind::Type {
            continue;
        }
        let Some(next) = tokens.get(index + 1) else {
            break;
        };
        let TokenKind::Ident(name) = next.kind else {
            continue;
        };
        type_names
            .push(name)
            .map_err(|_| crate::out_of_space(path, "", next.line, next.column, Resource::Arena))?;
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        path,
        unit: "",
        bump,
        type_names,
        nesting: 0,
    };
    parser.unit()
}

impl<'a, 'b> Parser<'a, 'b> {
    fn peek(&self) -> Token<'a> {
        self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    /// Enters one recursive-descent level, or rejects the source when the
    /// fixed limit is reached. The caller must pair every success with
    /// [`Self::leave`], including on error paths.
    fn enter(&mut self, line: u32, column: u32) -> Result<(), Diagnostic> {
        if self.nesting >= MAX_NESTING {
            return Err(self.error(
                line,
                column,
                format_args!("nesting too deep (limit {MAX_NESTING})"),
            ));
        }
        self.nesting += 1;
        Ok(())
    }

    fn leave(&mut self) {
        debug_assert!(self.nesting > 0);
        self.nesting -= 1;
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
        // Diagnostics for a bad import point at the clause itself (spec 0018,
        // M2), so remember where it starts. A unit without one keeps 1:1.
        let (mut uses_line, mut uses_column) = (1u32, 1u32);
        if self.check(&TokenKind::Uses) {
            let clause = self.peek();
            uses_line = clause.line;
            uses_column = clause.column;
            self.advance();
            loop {
                // A module import is a dotted name (`lib.ui.menu`); the parser
                // accepts the dotted shape and the compiler validates it.
                let (used, line, column) = self.name_path()?;
                self.push(&mut uses, used, line, column)?;
                if self.eat(&TokenKind::Comma) {
                    continue;
                }
                break;
            }
            self.expect(&TokenKind::Semi, "`;`")?;
        }

        let mut types = self.list::<TypeDecl<'a>>(start.line, start.column)?;
        let mut consts = self.list::<ConstDecl<'a>>(start.line, start.column)?;
        let mut vars = self.list::<VarDecl<'a>>(start.line, start.column)?;
        let mut procs = self.list::<ProcDecl<'a>>(start.line, start.column)?;

        loop {
            let token = self.peek();
            match token.kind {
                TokenKind::Type => self.type_section(&mut types)?,
                TokenKind::Const => self.const_section(&mut consts)?,
                TokenKind::Var => self.var_section(&mut vars)?,
                TokenKind::Procedure | TokenKind::Function => {
                    let proc = self.subprogram()?;
                    self.push(&mut procs, proc, token.line, token.column)?;
                }
                TokenKind::Eof => break,
                TokenKind::End => {
                    // A stray `end` most often means a missing `begin`.
                    return Err(self.here(format_args!("unexpected `end`; expected a declaration")));
                }
                other => {
                    return Err(self.here(format_args!(
                        "expected `type`, `const`, `var`, `procedure`, `function` or end of file, found {other}"
                    )))
                }
            }
        }

        Ok(Unit {
            name,
            uses: uses.into_slice(),
            uses_line,
            uses_column,
            types: types.into_slice(),
            consts: consts.into_slice(),
            vars: vars.into_slice(),
            procs: procs.into_slice(),
        })
    }

    /// Parses a `type` section. Three declaration forms are accepted:
    /// `type Name = (Variant, ...);` (enum),
    /// `type Name = record field: T; ... end;` (record) and
    /// `type Name = Variant | Variant(payload: T);` (sum type).
    fn type_section(
        &mut self,
        decls: &mut SliceVec<'a, 'b, TypeDecl<'a>>,
    ) -> Result<(), Diagnostic> {
        let start = self.peek();
        self.expect(&TokenKind::Type, "`type`")?;
        let mut count = 0;
        while matches!(self.peek().kind, TokenKind::Ident(_)) {
            let token = self.peek();
            let name = self.ident("a type name")?;
            self.expect(&TokenKind::Eq, "`=`")?;
            let kind = if self.eat(&TokenKind::Record) {
                let fields = self.record_fields()?;
                if fields.len() == 0 {
                    return Err(self.error(
                        token.line,
                        token.column,
                        format_args!("record `{name}` needs at least one field"),
                    ));
                }
                TypeKind::Record(fields)
            } else if self.check(&TokenKind::LParen) {
                self.advance();
                let mut variants = self.list::<&'a str>(token.line, token.column)?;
                if !self.check(&TokenKind::RParen) {
                    loop {
                        let variant = self.ident("a variant name")?;
                        self.push(&mut variants, variant, token.line, token.column)?;
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
                if variants.len() == 0 {
                    return Err(self.error(
                        token.line,
                        token.column,
                        format_args!("enum `{name}` needs at least one variant"),
                    ));
                }
                TypeKind::Enum(variants.into_slice())
            } else if matches!(self.peek().kind, TokenKind::Ident(_)) {
                let variants = self.sum_variants()?;
                TypeKind::Sum(variants)
            } else {
                let found = self.peek().kind;
                return Err(self.here(format_args!(
                    "expected `(`, `record` or a sum variant after `=`, found {found}"
                )));
            };
            self.expect(&TokenKind::Semi, "`;`")?;
            let decl = TypeDecl {
                name,
                kind,
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
                format_args!("`type` must be followed by at least one declaration"),
            ));
        }
        Ok(())
    }

    /// Parses sum variants: `Variant` or `Variant(field: type)`, separated by
    /// `|`.
    fn sum_variants(&mut self) -> Result<&'a [SumVariantDecl<'a>], Diagnostic> {
        let start = self.peek();
        let mut variants = self.list::<SumVariantDecl<'a>>(start.line, start.column)?;
        loop {
            let token = self.peek();
            let name = self.ident("a variant name")?;
            let payload = if self.eat(&TokenKind::LParen) {
                let field = self.ident("a payload name")?;
                self.expect(&TokenKind::Colon, "`:` after a payload name")?;
                let ty = self.type_ref()?;
                self.expect(&TokenKind::RParen, "`)`")?;
                Some((field, ty))
            } else {
                None
            };
            let decl = SumVariantDecl {
                name,
                payload,
                line: token.line,
                column: token.column,
            };
            self.push(&mut variants, decl, token.line, token.column)?;
            if !self.eat(&TokenKind::Pipe) {
                break;
            }
        }
        Ok(variants.into_slice())
    }

    /// Parses `field[, field]* : type;` lines up to `end`.
    fn record_fields(&mut self) -> Result<&'a [FieldDecl<'a>], Diagnostic> {
        let start = self.peek();
        let mut fields = self.list::<FieldDecl<'a>>(start.line, start.column)?;
        loop {
            if self.check(&TokenKind::End) {
                self.advance();
                break;
            }
            if self.check(&TokenKind::Eof) {
                return Err(self.here(format_args!("record body is missing `end`")));
            }
            if self.eat(&TokenKind::Semi) {
                continue;
            }
            let token = self.peek();
            let mut names = self.list::<&'a str>(token.line, token.column)?;
            let first = self.ident("a field name")?;
            self.push(&mut names, first, token.line, token.column)?;
            while self.eat(&TokenKind::Comma) {
                let name = self.ident("a field name")?;
                self.push(&mut names, name, token.line, token.column)?;
            }
            self.expect(&TokenKind::Colon, "`:`")?;
            let ty = self.type_ref()?;
            self.expect(&TokenKind::Semi, "`;`")?;
            for index in 0..names.len() {
                let name = names.as_slice()[index];
                let field = FieldDecl {
                    name,
                    ty,
                    line: token.line,
                    column: token.column,
                };
                self.push(&mut fields, field, token.line, token.column)?;
            }
        }
        Ok(fields.into_slice())
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
        self.enter(token.line, token.column)?;
        let result = self.type_ref_inner();
        self.leave();
        result
    }

    /// The body of [`Self::type_ref`]; array, ring and map element types recurse
    /// through the bounded wrapper, so a deeply nested `array[array[...]]`,
    /// `ring[ring[...]]` or `map[int, array[...], 4]` is a diagnostic rather
    /// than a stack fault.
    fn type_ref_inner(&mut self) -> Result<TypeRef<'a>, Diagnostic> {
        let token = self.peek();
        let first = self.ident("a type name")?;
        let mut owner = None;
        let mut name = first;
        // A module's type is named `<unit>.<Type>` (spec 0003, "Cross-unit
        // types"); the built-in parametric and buffer types stay unqualified
        // and reserved. Only one qualifier is legal: there is no
        // `a.b.Type` type name.
        if !matches!(
            first,
            "array" | "ring" | "map" | "string" | "bytes" | "in_stream" | "out_stream"
        ) && self.eat(&TokenKind::Dot)
        {
            owner = Some(first);
            name = self.ident("a type name after `.`")?;
        }
        let mut cap = None;
        let mut elem = None;
        let mut value = None;
        let mut count = None;
        if matches!(name, "array" | "ring" | "map") {
            self.expect(
                &TokenKind::LBracket,
                if name == "map" {
                    "`[` after `map`"
                } else if name == "array" {
                    "`[` after `array`"
                } else {
                    "`[` after `ring`"
                },
            )?;
            let first = self.type_ref()?;
            let first = self
                .bump
                .alloc(first)
                .map_err(|_| self.arena_error(token.line, token.column))?;
            elem = Some(&*first);
            self.expect(
                &TokenKind::Comma,
                if name == "map" {
                    "`,` between a map's key type and value type"
                } else if name == "array" {
                    "`,` between an array's element type and count"
                } else {
                    "`,` between a ring's element type and count"
                },
            )?;
            if name == "map" {
                let second = self.type_ref()?;
                let second = self
                    .bump
                    .alloc(second)
                    .map_err(|_| self.arena_error(token.line, token.column))?;
                value = Some(&*second);
                self.expect(
                    &TokenKind::Comma,
                    "`,` between a map's value type and count",
                )?;
            }
            count = Some(self.expression()?);
            self.expect(&TokenKind::RBracket, "`]`")?;
        } else if matches!(name, "string" | "bytes" | "in_stream" | "out_stream") {
            self.expect(&TokenKind::LBracket, "`[` after a buffer or stream type")?;
            cap = Some(self.expression()?);
            self.expect(&TokenKind::RBracket, "`]`")?;
        }
        Ok(TypeRef {
            owner,
            name,
            cap,
            elem,
            value,
            count,
            line: token.line,
            column: token.column,
        })
    }

    /// Parses `procedure Name(params);` or `function Name(params): T;`,
    /// followed by the optional locals and the body.
    fn subprogram(&mut self) -> Result<ProcDecl<'a>, Diagnostic> {
        let token = self.peek();
        let is_function = if self.eat(&TokenKind::Function) {
            true
        } else {
            self.expect(&TokenKind::Procedure, "`procedure` or `function`")?;
            false
        };
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
        let ret = if is_function {
            self.expect(&TokenKind::Colon, "`:` and a result type after a function")?;
            Some(self.type_ref()?)
        } else {
            None
        };
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
            ret,
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
        self.enter(token.line, token.column)?;
        let result = self.statement_inner();
        self.leave();
        result
    }

    fn statement_inner(&mut self) -> Result<&'a Stmt<'a>, Diagnostic> {
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
            TokenKind::Case => {
                self.advance();
                let value = self.expression()?;
                self.expect(&TokenKind::Of, "`of` after the `case` value")?;
                let arms = self.case_arms()?;
                self.expect(&TokenKind::End, "`end` after the `case` arms")?;
                self.alloc_stmt(Stmt::Case { value, arms }, token.line, token.column)
            }
            TokenKind::Return => {
                self.advance();
                let value = if self.check(&TokenKind::Semi) {
                    None
                } else {
                    Some(self.expression()?)
                };
                self.expect(&TokenKind::Semi, "`;` after `return`")?;
                self.alloc_stmt(
                    Stmt::Return {
                        value,
                        line: token.line,
                        column: token.column,
                    },
                    token.line,
                    token.column,
                )
            }
            TokenKind::Ident(_) => {
                let (target, line, column) = self.name_path()?;
                if self.check(&TokenKind::LParen) {
                    let call = self.call(target, line, column)?;
                    return self.alloc_stmt(Stmt::Call(call), line, column);
                }
                let (base, path) = self.designator_path(target, line, column)?;
                self.expect(&TokenKind::Assign, "`:=` after an assignment target")?;
                let value = self.expression()?;
                self.alloc_stmt(
                    Stmt::Assign {
                        target: Designator { base, path },
                        value,
                        line,
                        column,
                    },
                    line,
                    column,
                )
            }
            other => Err(self.here(format_args!("expected a statement, found {other}"))),
        }
    }

    /// True when the next tokens begin a `case` arm: `Ident:` or
    /// `Ident(Ident):`. An arm body is otherwise a statement sequence, so this
    /// lookahead is what separates one arm from the next.
    fn case_arm_start(&self) -> bool {
        let kind_at = |index: usize| self.tokens.get(index).map(|token| token.kind);
        if !matches!(kind_at(self.pos), Some(TokenKind::Ident(_))) {
            return false;
        }
        if matches!(kind_at(self.pos + 1), Some(TokenKind::LParen)) {
            if !matches!(kind_at(self.pos + 2), Some(TokenKind::Ident(_))) {
                return false;
            }
            if !matches!(kind_at(self.pos + 3), Some(TokenKind::RParen)) {
                return false;
            }
            return matches!(kind_at(self.pos + 4), Some(TokenKind::Colon));
        }
        matches!(kind_at(self.pos + 1), Some(TokenKind::Colon))
    }

    /// Parses `case` arms up to (but not including) `end`.
    fn case_arms(&mut self) -> Result<&'a [CaseArm<'a>], Diagnostic> {
        let start = self.peek();
        let mut arms = self.list::<CaseArm<'a>>(start.line, start.column)?;
        loop {
            if self.check(&TokenKind::End) {
                break;
            }
            if self.check(&TokenKind::Eof) {
                return Err(self.here(format_args!("`case` is missing `end`")));
            }
            if self.eat(&TokenKind::Semi) {
                continue;
            }
            if !self.case_arm_start() {
                let found = self.peek().kind;
                return Err(self.here(format_args!(
                    "expected a `case` arm (`Variant:` or `Variant(name):`), found {found}"
                )));
            }
            let token = self.peek();
            let variant = self.ident("a variant name")?;
            let binding = if self.eat(&TokenKind::LParen) {
                let binding = self.ident("a payload binding name")?;
                self.expect(&TokenKind::RParen, "`)`")?;
                Some(binding)
            } else {
                None
            };
            self.expect(&TokenKind::Colon, "`:` after a `case` arm")?;
            let mut body = self.list::<Stmt<'a>>(token.line, token.column)?;
            loop {
                if self.check(&TokenKind::End)
                    || self.check(&TokenKind::Eof)
                    || self.case_arm_start()
                {
                    break;
                }
                if self.eat(&TokenKind::Semi) {
                    continue;
                }
                let at = self.peek();
                let stmt = self.statement()?;
                self.push(&mut body, *stmt, at.line, at.column)?;
            }
            let arm = CaseArm {
                variant,
                binding,
                body: body.into_slice(),
                line: token.line,
                column: token.column,
            };
            self.push(&mut arms, arm, token.line, token.column)?;
        }
        if arms.len() == 0 {
            return Err(self.error(
                start.line,
                start.column,
                format_args!("`case` needs at least one arm"),
            ));
        }
        Ok(arms.into_slice())
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

    /// Builds the designator path after a base name.
    ///
    /// A dotted name (`p.x.other`) contributes one `Field` per part; then any
    /// postfix `[index]` and `.field` steps are appended. The path is arena
    /// data, so a designator is `Copy` and can be referenced repeatedly.
    fn designator_path(
        &mut self,
        name: &'a str,
        line: u32,
        column: u32,
    ) -> Result<(&'a str, &'a [Access<'a>]), Diagnostic> {
        let mut parts = self.list::<Access<'a>>(line, column)?;
        let mut names = name.split('.');
        let base = names.next().unwrap_or(name);
        if base.is_empty() {
            return Err(self.error(line, column, format_args!("malformed field access")));
        }
        for part in names {
            if part.is_empty() {
                return Err(self.error(line, column, format_args!("malformed field access")));
            }
            self.push(&mut parts, Access::Field(part), line, column)?;
        }
        loop {
            if self.eat(&TokenKind::LBracket) {
                let index = self.expression()?;
                self.expect(&TokenKind::RBracket, "`]`")?;
                self.push(&mut parts, Access::Index(index), line, column)?;
            } else if self.eat(&TokenKind::Dot) {
                let part = self.ident("a name after `.`")?;
                self.push(&mut parts, Access::Field(part), line, column)?;
            } else {
                break;
            }
        }
        Ok((base, parts.into_slice()))
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

    // The precedence chain is marked `inline(always)` so a nested expression
    // carries one merged frame per level instead of eight chained frames. This
    // nearly halves the stack cost of a nesting level (Q42).
    fn expression(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
        let token = self.peek();
        self.enter(token.line, token.column)?;
        let result = self.parse_or();
        self.leave();
        result
    }

    fn binary(
        &self,
        left: &'a Expr<'a>,
        op: BinOp,
        right: &'a Expr<'a>,
    ) -> Result<&'a Expr<'a>, Diagnostic> {
        self.alloc_expr(ExprKind::Binary(op, left, right), left.line, left.column)
    }

    #[inline(always)]
    fn parse_or(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
        let mut left = self.parse_and()?;
        while self.eat(&TokenKind::Or) {
            let right = self.parse_and()?;
            left = self.binary(left, BinOp::Or, right)?;
        }
        Ok(left)
    }

    #[inline(always)]
    fn parse_and(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
        let mut left = self.parse_equality()?;
        while self.eat(&TokenKind::And) {
            let right = self.parse_equality()?;
            left = self.binary(left, BinOp::And, right)?;
        }
        Ok(left)
    }

    #[inline(always)]
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

    #[inline(always)]
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

    #[inline(always)]
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

    #[inline(always)]
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

    #[inline(always)]
    fn parse_unary(&mut self) -> Result<&'a Expr<'a>, Diagnostic> {
        let token = self.peek();
        if self.eat(&TokenKind::Minus) {
            self.enter(token.line, token.column)?;
            let operand = self.parse_unary();
            self.leave();
            let operand = operand?;
            return self.alloc_expr(
                ExprKind::Unary(UnOp::Neg, operand),
                token.line,
                token.column,
            );
        }
        if self.eat(&TokenKind::Not) {
            self.enter(token.line, token.column)?;
            let operand = self.parse_unary();
            self.leave();
            let operand = operand?;
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
                    let is_cast = !name.contains('.')
                        && (matches!(
                            name,
                            "int"
                                | "uint"
                                | "byte"
                                | "f64"
                                | "Status"
                                | "FaultCode"
                                | "Diagnostic"
                                | "ExitReason"
                        ) || self.type_names.as_slice().contains(&name));
                    if is_cast {
                        self.advance();
                        let inner = self.expression()?;
                        self.expect(&TokenKind::RParen, "`)`")?;
                        return self.alloc_expr(ExprKind::Cast(name, inner), line, column);
                    }
                    let call = self.call(name, line, column)?;
                    return self.alloc_expr(ExprKind::Call(call), line, column);
                }
                let (base, path) = self.designator_path(name, line, column)?;
                self.alloc_expr(
                    ExprKind::Designator(Designator { base, path }),
                    line,
                    column,
                )
            }
            other => Err(self.here(format_args!("expected an expression, found {other}"))),
        }
    }
}
