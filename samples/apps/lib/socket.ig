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
// Sockets are non-blocking in this first slice: `send`/`recv` return
// `WouldBlock` until the stack makes progress, and the caller retries with its
// own `core.clock()` deadline. A send copies at most one TCP segment; a small
// request fits in one call, and a larger buffer is an application choice.
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
