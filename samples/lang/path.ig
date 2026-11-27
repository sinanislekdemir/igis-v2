unit path_check;

// Host-side check for `lib.path` (spec 0018 modules). It exercises the four
// public helpers on the `langhost` runner:
//
//   cargo run -p langhost -- samples/lang/path.ig
//
// The sample is not staged into the kernel image; the kernel proof of the
// commands that use the module is `run ls /`, `run cat ...` and so on on the
// shell image.

uses core, stdlib, lib.path;

var
  ok: bool;
  cwd: string[256];
  rest: string[256];
  resolved: string[256];
  name: string[64];
  st: Status;

procedure expect(label: const string[0];
                 value: const string[0];
                 wanted: const string[0]);
begin
  if value = wanted then
    core.write("ok ")
  else
  begin
    core.write("FAIL ");
    ok := false;
  end;
  core.write(label);
  core.write("=");
  core.write(value);
  core.write("\n");
end;

procedure expect_status(label: const string[0]; got: Status; wanted: Status);
begin
  if got = wanted then
    core.write("ok ")
  else
  begin
    core.write("FAIL ");
    ok := false;
  end;
  core.write(label);
  core.write("\n");
end;

procedure main(args: const string[1024]);
begin
  ok := true;

  st := path.split_cwd("", cwd, rest);
  expect_status("split0 status", st, Ok);
  expect("split0 cwd", cwd, "/");
  expect("split0 rest", rest, "");

  st := path.split_cwd("/base\n", cwd, rest);
  expect_status("split1 status", st, Ok);
  expect("split1 cwd", cwd, "/base");
  expect("split1 rest", rest, "");

  st := path.split_cwd("/base\nls -a", cwd, rest);
  expect_status("split2 status", st, Ok);
  expect("split2 cwd", cwd, "/base");
  expect("split2 rest", rest, "ls -a");

  st := path.split_cwd("base/child", cwd, rest);
  expect_status("split3 status", st, Ok);
  expect("split3 cwd", cwd, "/");
  expect("split3 rest", rest, "base/child");

  st := path.split_cwd("/home\n  note.txt  ", cwd, rest);
  expect_status("split4 status", st, Ok);
  expect("split4 cwd", cwd, "/home");
  expect("split4 rest", rest, "note.txt");

  st := path.join("/", "base/child", resolved);
  expect_status("join1 status", st, Ok);
  expect("join1", resolved, "base/child");

  st := path.join("/base", "", resolved);
  expect_status("join2 status", st, Ok);
  expect("join2", resolved, "base");

  st := path.join("/base", "..", resolved);
  expect_status("join3 status", st, Ok);
  expect("join3", resolved, "");

  st := path.join("/base", "../..", resolved);
  expect_status("join4 status", st, Ok);
  expect("join4", resolved, "");

  st := path.join("/", "/lib", resolved);
  expect_status("join5 status", st, Ok);
  expect("join5", resolved, "lib");

  st := path.join("/home", "~/x", resolved);
  expect_status("join6 status", st, Ok);
  expect("join6", resolved, "home/x");

  st := path.join("/home", "~", resolved);
  expect_status("join7 status", st, Ok);
  expect("join7", resolved, "home");

  st := path.join("/base", "./x/../y", resolved);
  expect_status("join8 status", st, Ok);
  expect("join8", resolved, "base/y");

  st := path.join("/base", "a//b", resolved);
  expect_status("join9 status", st, Ok);
  expect("join9", resolved, "base/a/b");

  st := path.join("/", "../../home/x", resolved);
  expect_status("join10 status", st, Ok);
  expect("join10", resolved, "home/x");

  st := path.display("", resolved);
  expect_status("display0 status", st, Ok);
  expect("display0", resolved, "/");

  st := path.display("base/child", resolved);
  expect_status("display1 status", st, Ok);
  expect("display1", resolved, "/base/child");

  st := path.display("/base", resolved);
  expect_status("display2 status", st, Ok);
  expect("display2", resolved, "/base");

  st := path.basename("base/child", name);
  expect_status("base0 status", st, Ok);
  expect("base0", name, "child");

  st := path.basename("", name);
  expect_status("base1 status", st, Ok);
  expect("base1", name, "");

  st := path.basename("base", name);
  expect_status("base2 status", st, Ok);
  expect("base2", name, "base");

  if ok then
    core.write("path: ok\n")
  else
    core.write("path: FAILED\n");
end;
