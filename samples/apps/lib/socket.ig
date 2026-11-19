unit socket;

// Importable socket unit (spec 0020, N12–N15; ADR 0050). It is ordinary `/lib`
// module source: no `main`, no top-level `var` state, no stream endpoint and no
// kernel service. Every function maps to one `core.socket_*` host builtin and
// keeps no state, so the wire stack stays in the kernel and the application
// owns only the opaque, owner-checked handle.
//
//   value := socket.tcp(uint(0x0A00020F), uint(6666), id);
//   ...
//   st := socket.send_text(id, request, sent);
//   st := socket.recv(id, chunk, read);
//   socket.close(id);
//
// Sockets block in the scheduler: `send`/`recv`/`accept` return `WouldBlock`
// only when `socket.set_timeout` armed a deadline that expired (or when the
// host has no network path); otherwise the task sleeps on the socket and the
// same call resumes when data/space/EOF/close arrives. A send copies at most
// one TCP segment; a small request fits in one call, and a larger buffer is an
// application choice.
//
// The module owns no state and adds no opcode, builtin or `state`: the fixed
// receive destination is the language's answer to a caller-owned capacity,
// and a task that needs another size calls `core.socket_recv` directly.

uses core, stdlib;

// Opens a TCP client connection to `ip`:`port`. The stack starts the handshake
// and returns at once; `send` is `WouldBlock` until it is established. `id`
// receives the handle only on `Ok`.
function tcp(ip: uint; port: uint; id: var int): Status;
begin
  return core.socket_open(uint(1), ip, port, uint(0), id);
end;

// Opens a connected UDP socket. A send is one datagram; `close` frees the slot
// at once (there is no close handshake).
function udp(ip: uint; port: uint; id: var int): Status;
begin
  return core.socket_open(uint(2), ip, port, uint(0), id);
end;

// Opens a connected UDP socket on an explicit local port. DHCP needs this:
// it answers from port 68 and must therefore send from 68. The same port
// collision rules apply, so a second socket on `local` is `Invalid`.
function udp_from(local: uint; ip: uint; port: uint; id: var int): Status;
begin
  return core.socket_open(uint(2), ip, port, local, id);
end;

// Opens a passive TCP listener bound to `port` (spec 0020, N10). The stack
// answers inbound SYNs at once; `accept` pops one completed connection. `id`
// receives the handle only on `Ok`; `port` must be nonzero and a second
// listener on the same port is `Invalid`.
function listen(port: uint; id: var int): Status;
begin
  return core.socket_open(uint(3), uint(0), uint(0), port, id);
end;

// Pops the oldest completed connection from a listener. `peer` receives a
// normal connected socket handle owned by the calling task, so `send`, `recv`,
// `send_text` and `close` work on it unchanged. `WouldBlock` means no
// connection is pending (the caller retries); `peer` is untouched then.
function accept(id: int; peer: var int): Status;
begin
  return core.socket_accept(id, peer);
end;

// Closes a socket owned by the calling task.
function close(id: int): Status;
begin
  return core.socket_close(id);
end;

// Sets the deadline for the next blocking send/receive/accept (milliseconds;
// `uint(0)` blocks indefinitely). A blocked call that expires returns
// `WouldBlock` without sleeping again, so the caller can observe the timeout;
// the kernel stack clamps a value above one hour. `set_timeout` itself is
// `Ok` even when no call follows.
function set_timeout(id: int; ms: uint): Status;
begin
  return core.socket_timeout(id, ms);
end;

// The `core.next_any`/`poll_any` source for a socket: `1 << 63 | handle`.
// Readiness means data, EOF, an error, a completed connect or (for a listener)
// a pending connection. It is compiler-lowered, so it costs one load and an
// unsigned add; do not pass it to a stream primitive.
function source(id: int): uint;
begin
  return core.socket_source(id);
end;

// Sends up to one TCP segment (or one UDP datagram) from a caller-owned buffer.
// `sent` receives the copied prefix length only on `Ok`; `WouldBlock` leaves it
// untouched so the caller can retry the same buffer.
function send(id: int; data: const bytes[0]; sent: var int): Status;
begin
  return core.socket_send(id, data, sent);
end;

// Sends a whole `string` by staging it into a 1024-byte local buffer (one
// checked `core.substr_into`, so the wire never sees a partial code point). A
// string larger than the staging buffer is `CapacityExceeded` and sends
// nothing. This is the small-request path; a larger payload uses `send` with a
// caller-owned `bytes[N]`.
function send_text(id: int; text: const string[0]; sent: var int): Status;
var
  stage: bytes[1024];
  length: int;
  copied: int;
begin
  length := int(core.len(text));
  if length > 1024 then
    return CapacityExceeded;
  copied := core.substr_into(stage, text, 0, length);
  if copied < 0 then
    return Invalid;
  return send(id, stage, sent);
end;

// Receives up to 1024 bytes into the caller's buffer. `read` receives the count
// only on `Ok`; `Ok` with read 0 is end of file (a TCP FIN), and `WouldBlock`
// leaves `dst` and `read` untouched. A caller with another capacity calls
// `core.socket_recv` directly.
function recv(id: int; dst: var bytes[1024]; read: var int): Status;
begin
  return core.socket_recv(id, dst, read);
end;
