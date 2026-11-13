//! Abstract syntax tree produced by the parser.
//!
//! The tree is deliberately close to the source: names are unresolved and
//! capacities are expressions. `codegen` resolves names, evaluates capacities
//! and emits bytecode. Keeping the two phases apart is what makes the
//! diagnostics point at the right token.

/// One source file.
#[derive(Debug, Clone)]
pub struct Unit {
    /// Name from the `unit <name>;` declaration.
    pub name: String,
    /// Names in the optional `uses ...;` clause.
    pub uses: Vec<String>,
    /// Top-level constants, in declaration order.
    pub consts: Vec<ConstDecl>,
    /// Top-level variables, in declaration order.
    pub vars: Vec<VarDecl>,
    /// Procedures, in declaration order. The order fixes the function indices.
    pub procs: Vec<ProcDecl>,
}

/// `const Name = <expression>;`
#[derive(Debug, Clone)]
pub struct ConstDecl {
    /// Constant name.
    pub name: String,
    /// Value expression; evaluated at compile time.
    pub value: Expr,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// `name: type;`
#[derive(Debug, Clone)]
pub struct VarDecl {
    /// Variable name.
    pub name: String,
    /// Declared type.
    pub ty: TypeRef,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// A written type such as `int` or `string[64]`.
#[derive(Debug, Clone)]
pub struct TypeRef {
    /// Type name, before resolution.
    pub name: String,
    /// Capacity expression for `string`/`bytes`.
    pub cap: Option<Expr>,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// Parameter passing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    /// Value copy (the default).
    Value,
    /// Read-only call-scoped borrow.
    Const,
    /// Writable call-scoped borrow.
    Var,
}

/// One parameter declaration.
#[derive(Debug, Clone)]
pub struct ParamDecl {
    /// Parameter name.
    pub name: String,
    /// Passing mode.
    pub mode: ParamMode,
    /// Declared type.
    pub ty: TypeRef,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// `procedure Name(...); <declarations> begin ... end;`
#[derive(Debug, Clone)]
pub struct ProcDecl {
    /// Procedure name.
    pub name: String,
    /// Parameters, in order.
    pub params: Vec<ParamDecl>,
    /// Procedure-local variables.
    pub locals: Vec<VarDecl>,
    /// Body statements.
    pub body: Vec<Stmt>,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// A statement.
#[derive(Debug, Clone)]
pub enum Stmt {
    /// `target := value;`
    Assign {
        /// Destination name.
        target: String,
        /// Right-hand side.
        value: Expr,
        /// 1-based source line.
        line: u32,
        /// 1-based source column.
        column: u32,
    },
    /// A call used as a statement.
    Call(Call),
    /// `if condition then then_branch [else else_branch]`
    If {
        /// Boolean condition.
        cond: Expr,
        /// Branch taken when the condition is true.
        then_branch: Box<Stmt>,
        /// Branch taken otherwise, if any.
        else_branch: Option<Box<Stmt>>,
    },
    /// `while condition do body`
    While {
        /// Boolean condition.
        cond: Expr,
        /// Loop body.
        body: Box<Stmt>,
    },
    /// `begin ... end`
    Block(Vec<Stmt>),
}

/// A call expression or call statement.
#[derive(Debug, Clone)]
pub struct Call {
    /// Callee name, possibly qualified (`core.write`).
    pub target: String,
    /// Arguments, in order.
    pub args: Vec<Expr>,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// An expression.
#[derive(Debug, Clone)]
pub struct Expr {
    /// Expression form.
    pub kind: ExprKind,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// Expression forms.
#[derive(Debug, Clone)]
pub enum ExprKind {
    /// Integer literal.
    Int(i64),
    /// Floating-point literal.
    Float(f64),
    /// String literal, as raw bytes.
    Str(Vec<u8>),
    /// `true` or `false`.
    Bool(bool),
    /// A name.
    Ident(String),
    /// A call.
    Call(Call),
    /// A binary operation.
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// A unary operation.
    Unary(UnOp, Box<Expr>),
    /// An explicit scalar conversion such as `int(x)`.
    Cast(String, Box<Expr>),
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/` (floats only)
    Slash,
    /// `div`
    Div,
    /// `mod`
    Mod,
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
    /// `and`
    And,
    /// `or`
    Or,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    /// `-`
    Neg,
    /// `not`
    Not,
}
