//! Abstract syntax tree produced by the parser.
//!
//! The tree is deliberately close to the source: names are unresolved and
//! capacities are expressions. `codegen` resolves names, evaluates capacities
//! and emits bytecode. Keeping the two phases apart is what makes the
//! diagnostics point at the right token.
//!
//! Every node is `Copy` and lives in the caller's arena: names and literals
//! borrow the source (or decoded literal bytes) and children are references to
//! arena slots, so the tree has no ownership and nothing to free.

/// One source file.
#[derive(Debug, Clone, Copy)]
pub struct Unit<'a> {
    /// Name from the `unit <name>;` declaration.
    pub name: &'a str,
    /// Names in the optional `uses ...;` clause.
    pub uses: &'a [&'a str],
    /// Top-level `type` declarations, in declaration order.
    pub types: &'a [TypeDecl<'a>],
    /// Top-level constants, in declaration order.
    pub consts: &'a [ConstDecl<'a>],
    /// Top-level variables, in declaration order.
    pub vars: &'a [VarDecl<'a>],
    /// Procedures, in declaration order. The order fixes the function indices.
    pub procs: &'a [ProcDecl<'a>],
}

/// `type Name = (Variant, ...);`, `type Name = record field: T; end;` or
/// `type Name = Variant | Variant(payload: T);`
///
/// A `type` declaration is an enum, a record or a sum type.
#[derive(Debug, Clone, Copy)]
pub struct TypeDecl<'a> {
    /// Type name.
    pub name: &'a str,
    /// Enum variants, record fields or sum variants.
    pub kind: TypeKind<'a>,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// The body of a `type` declaration.
#[derive(Debug, Clone, Copy)]
pub enum TypeKind<'a> {
    /// `(Variant, Variant, ...)`: variants in declaration order, ordinals
    /// `0..n-1`.
    Enum(&'a [&'a str]),
    /// `record field: T; other: T; end;`: fields in declaration order.
    Record(&'a [FieldDecl<'a>]),
    /// `Variant | Variant(payload: T) | ...`: variants in declaration order,
    /// tags `0..n-1` (spec 0003, "Sum types").
    Sum(&'a [SumVariantDecl<'a>]),
}

/// One sum-type variant: `Name` or `Name(field: T)`.
#[derive(Debug, Clone, Copy)]
pub struct SumVariantDecl<'a> {
    /// Variant name.
    pub name: &'a str,
    /// Optional single payload: field name and declared type.
    pub payload: Option<(&'a str, TypeRef<'a>)>,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// One record field: `name: type;`. A grouped declaration `x, y: int;` becomes
/// one `FieldDecl` per name.
#[derive(Debug, Clone, Copy)]
pub struct FieldDecl<'a> {
    /// Field name.
    pub name: &'a str,
    /// Declared type.
    pub ty: TypeRef<'a>,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// `const Name = <expression>;`
#[derive(Debug, Clone, Copy)]
pub struct ConstDecl<'a> {
    /// Constant name.
    pub name: &'a str,
    /// Value expression; evaluated at compile time.
    pub value: &'a Expr<'a>,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// `name: type;`
#[derive(Debug, Clone, Copy)]
pub struct VarDecl<'a> {
    /// Variable name.
    pub name: &'a str,
    /// Declared type.
    pub ty: TypeRef<'a>,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// A written type such as `int`, `string[64]` or `array[int, 4]`.
#[derive(Debug, Clone, Copy)]
pub struct TypeRef<'a> {
    /// Type name, before resolution.
    pub name: &'a str,
    /// Capacity expression for `string`/`bytes`.
    pub cap: Option<&'a Expr<'a>>,
    /// Element type for `array[T, N]`.
    pub elem: Option<&'a TypeRef<'a>>,
    /// Element count expression for `array[T, N]`.
    pub count: Option<&'a Expr<'a>>,
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
#[derive(Debug, Clone, Copy)]
pub struct ParamDecl<'a> {
    /// Parameter name.
    pub name: &'a str,
    /// Passing mode.
    pub mode: ParamMode,
    /// Declared type.
    pub ty: TypeRef<'a>,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// `procedure Name(...); <declarations> begin ... end;` or
/// `function Name(...): T; <declarations> begin ... end;`
#[derive(Debug, Clone, Copy)]
pub struct ProcDecl<'a> {
    /// Procedure or function name.
    pub name: &'a str,
    /// Parameters, in order.
    pub params: &'a [ParamDecl<'a>],
    /// Result type: `Some` for a `function`, `None` for a `procedure`.
    pub ret: Option<TypeRef<'a>>,
    /// Procedure-local variables.
    pub locals: &'a [VarDecl<'a>],
    /// Body statements.
    pub body: &'a [Stmt<'a>],
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// A statement.
#[derive(Debug, Clone, Copy)]
pub enum Stmt<'a> {
    /// `target := value;`
    Assign {
        /// Destination designator: a name, a field path, or an array element.
        target: Designator<'a>,
        /// Right-hand side.
        value: &'a Expr<'a>,
        /// 1-based source line.
        line: u32,
        /// 1-based source column.
        column: u32,
    },
    /// A call used as a statement.
    Call(Call<'a>),
    /// `if condition then then_branch [else else_branch]`
    If {
        /// Boolean condition.
        cond: &'a Expr<'a>,
        /// Branch taken when the condition is true.
        then_branch: &'a Stmt<'a>,
        /// Branch taken otherwise, if any.
        else_branch: Option<&'a Stmt<'a>>,
    },
    /// `while condition do body`
    While {
        /// Boolean condition.
        cond: &'a Expr<'a>,
        /// Loop body.
        body: &'a Stmt<'a>,
    },
    /// `case value of Variant: ... Variant(binding): ... end`
    Case {
        /// Scrutinee: a variable or field of a sum type.
        value: &'a Expr<'a>,
        /// Arms in source order; exhaustive over the sum type's variants.
        arms: &'a [CaseArm<'a>],
    },
    /// `return value;` (a function) or `return;` (a procedure).
    Return {
        /// Result expression, absent in a procedure.
        value: Option<&'a Expr<'a>>,
        /// 1-based source line.
        line: u32,
        /// 1-based source column.
        column: u32,
    },
    /// `begin ... end`
    Block(&'a [Stmt<'a>]),
}

/// One `case` arm: `Variant: statements` or `Variant(binding): statements`.
#[derive(Debug, Clone, Copy)]
pub struct CaseArm<'a> {
    /// Variant name, resolved against the scrutinee's sum type.
    pub variant: &'a str,
    /// Name the payload is bound to inside the arm, if the variant carries one.
    pub binding: Option<&'a str>,
    /// Statements of the arm; no fallthrough.
    pub body: &'a [Stmt<'a>],
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// One step in a designator's path: a record field or an array/buffer element.
#[derive(Debug, Clone, Copy)]
pub enum Access<'a> {
    /// `.field`
    Field(&'a str),
    /// `[index]`
    Index(&'a Expr<'a>),
}

/// A variable, field path or indexed element: `name`, `p.x`, `a[i][j]`.
///
/// An empty path is a bare name. Field steps select record fields; index steps
/// select array elements (or, in the last position, one `byte` of a buffer).
/// The parser never builds a `Field` whose base is not a name, so every
/// designator stays rooted at one variable or parameter.
#[derive(Debug, Clone, Copy)]
pub struct Designator<'a> {
    /// Base variable or parameter name.
    pub base: &'a str,
    /// Steps after the name, in source order.
    pub path: &'a [Access<'a>],
}

impl Designator<'_> {
    /// True for a bare name with no field or index steps.
    pub fn is_name(&self) -> bool {
        self.path.is_empty()
    }
}

/// A call expression or call statement.
#[derive(Debug, Clone, Copy)]
pub struct Call<'a> {
    /// Callee name, possibly qualified (`core.write`).
    pub target: &'a str,
    /// Arguments, in order.
    pub args: &'a [&'a Expr<'a>],
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// An expression.
#[derive(Debug, Clone, Copy)]
pub struct Expr<'a> {
    /// Expression form.
    pub kind: ExprKind<'a>,
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// Expression forms.
#[derive(Debug, Clone, Copy)]
pub enum ExprKind<'a> {
    /// Integer literal.
    Int(i64),
    /// Floating-point literal.
    Float(f64),
    /// String literal, as raw bytes decoded into the arena.
    Str(&'a [u8]),
    /// `true` or `false`.
    Bool(bool),
    /// A variable, field path or indexed element.
    Designator(Designator<'a>),
    /// A call.
    Call(Call<'a>),
    /// A binary operation.
    Binary(BinOp, &'a Expr<'a>, &'a Expr<'a>),
    /// A unary operation.
    Unary(UnOp, &'a Expr<'a>),
    /// An explicit scalar conversion such as `int(x)`.
    Cast(&'a str, &'a Expr<'a>),
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
