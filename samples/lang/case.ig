unit sums;

// Sum types and `case` (spec 0003, "Sum types"):
// - construction is destination-driven: `token := Number(42)`.
// - `case` is exhaustive and binds a payload by value to a new local.
// - a whole-sum assignment copies tag and payload bytes.
// - a sum may be a record field or a parameter (`value`, `const`, `var`).

uses core;

type Token = Ident(name: string[8]) | Number(value: int) | Eof;
type Expr = Literal(value: int) | Nested(inner: Token) | Empty;
type Holder = record item: Token; end;

var
  token: Token;
  other: Token;
  expr: Expr;
  h: Holder;
  text: string[16];

// `const` sum parameter: read-only call-scoped reference.
procedure show(t: const Token);
begin
  case t of
    Ident(name):
    begin
      core.write("ident ");
      core.write(name);
      core.write("\n");
    end
    Number(value):
    begin
      core.write("number ");
      core.format_int_into(text, value, uint(10), uint(0));
      core.write(text);
      core.write("\n");
    end
    Eof: core.write("eof\n")
  end;
end;

// `var` sum parameter: assignment writes through the caller's reference.
procedure clear(t: var Token);
begin
  t := Eof;
end;

// `value` sum parameter: a copy, so the caller is untouched.
procedure wipe(t: Token);
begin
  t := Eof;
end;

procedure main(args: const string[1024]);
begin
  token := Number(42);
  show(token);
  other := token;
  token := Ident("igis");
  show(other);
  show(token);
  token := Eof;
  show(token);

  // The payload binding is a local copy: rebuilding it leaves the sum alone.
  token := Ident("keep");
  case token of
    Ident(name):
    begin
      name := "changed";
      show(token);
    end
    Number(value): core.write("wrong\n")
    Eof: core.write("wrong\n")
  end;
  show(token);

  // Nested `case`: a sum payload that is itself a sum.
  other := Number(7);
  expr := Nested(other);
  case expr of
    Literal(value): core.write("literal\n")
    Nested(inner):
      case inner of
        Ident(name): core.write("nested ident\n")
        Number(value):
        begin
          core.write("nested number ");
          core.format_int_into(text, value, uint(10), uint(0));
          core.write(text);
          core.write("\n");
        end
        Eof: core.write("nested eof\n")
      end
    Empty: core.write("empty\n")
  end;

  // A sum as a record field, then a `var` parameter over that field.
  h.item := Number(3);
  case h.item of
    Ident(name): core.write("h: ident\n")
    Number(value):
    begin
      core.write("h: number ");
      core.format_int_into(text, value, uint(10), uint(0));
      core.write(text);
      core.write("\n");
    end
    Eof: core.write("h: eof\n")
  end;

  clear(h.item);
  case h.item of
    Ident(name): core.write("var: ident\n")
    Number(value): core.write("var: number\n")
    Eof: core.write("var: eof\n")
  end;

  // `value` copies, so `other` is still `Number(7)`.
  wipe(other);
  show(other);
end;
