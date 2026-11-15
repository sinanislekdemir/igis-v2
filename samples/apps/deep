unit deep;

// Kernel sample for the declared-stack bound (spec 0002 H2, spec 0004 S7).
//
// `down` has no base case: every call pushes one interpreter frame and asks for
// the next. The interpreter checks each frame extent against the manifest's
// declared stack length and terminates the task with `Faulted(StackOverflow)`
// when the frame no longer fits. The task's hardware guard page sits below the
// region as a backstop, so this program proves the *legal* overflow path never
// reaches the page fault: the shell reports `child: faulted StackOverflow`.

uses core;

var
  depth: int;

function down(n: int): int;
begin
  return down(n + 1) + 1;
end;

procedure main(args: const string[1024]);
begin
  core.write("deep: recursing until the declared stack is exhausted\n");
  depth := down(0);
  core.write("deep: returned (the stack check did not fire)\n");
end;
