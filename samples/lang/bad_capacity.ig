unit bad_capacity;

var
  name: string[4];

procedure main(args: const string[1024]);
begin
  name := "too long";
end;
