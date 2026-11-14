//! The bytecode interpreter.
//!
//! The machine owns no memory: `state`, `stack` and the argument buffer are
//! caller-provided slices, and every access is bounds-checked against the
//! region that the 64-bit reference names. The operand stack and locals live
//! inside the caller's stack slice.
//!
//! # Frame layout
//!
//! ```text
//!   fp +  0  prev_fp: u64      (u64::MAX marks the outermost frame)
//!   fp +  8  return_pc: u32
//!   fp + 12  function: u32
//!   fp + 16  return_sp: u64
//!   fp + 24  dest_ref: u64
//!   fp + 32  locals and inline buffers
//!   fp + operand_off  operand stack
//!   fp + frame_size   end
//! ```
//!
//! # Safe points
//!
//! [`Machine::run`] takes a `budget` of instructions. It checks the budget and
//! any pending [`Machine::request_yield`] at every instruction boundary (which
//! is stronger than spec 0008's per-branch poll bit). When it returns
//! [`Outcome::Yielded`], the machine has not executed the next instruction and
//! can be resumed by calling `run` again with the same buffers. The scheduler
//! uses this to release a task, and a kill request is delivered by setting the
//! yield flag and waiting for the next `Yielded`.

use abi::layout;
use abi::opcodes::{self, builtin, value_kind};
use abi::{refs, ExitReason, FaultCode, Status};

use crate::program::Program;

/// The host interface as far as this slice needs it (spec 0005).
///
/// The kernel implements this over its own console, clock and REPL service;
/// the host toolchain implements it over stdout, `Instant` and a synchronous
/// child runner. Neither the interpreter nor the compiler knows which one is
/// behind the trait.
///
/// `spawn`, `wait` and `kill` have default implementations that refuse with
/// [`Status::Invalid`], so a test host that only needs the console and the
/// clock stays small (spec 0005, H4).
pub trait Host {
    /// Writes application bytes to the console. The bytes are unvalidated:
    /// they may be arbitrary binary data.
    fn console_write(&mut self, bytes: &[u8]);

    /// Pops one completed console line into `buf` (`core.read_line`).
    ///
    /// Returns the number of bytes copied, at most `buf.len()`, or `None` when
    /// no line is complete. The machine then returns [`Outcome::Blocked`] with
    /// [`WaitKind::Console`] without advancing and retries the same instruction
    /// when the scheduler wakes the task. The default is a host with no input,
    /// so tests that only need the console and the clock stay small.
    fn read_line(&mut self, _buf: &mut [u8]) -> Option<usize> {
        None
    }

    /// Monotonic nanoseconds since boot (or since the host process started).
    fn clock(&mut self) -> u64;

    /// Reads one byte from the x86 port space (`syslib.port_in8`).
    ///
    /// `Port` is staged as a scalar `uint` for now (Q45); the machine passes the
    /// low 16 bits to this method. The default returns zero, so a host mock or a
    /// test with no hardware sees deterministic zeroes.
    fn port_in8(&mut self, _port: u16) -> u8 {
        0
    }

    /// Writes one byte to the x86 port space (`syslib.port_out8`).
    ///
    /// The default is a no-op; a test host overrides it to record writes.
    fn port_out8(&mut self, _port: u16, _value: u8) {}

    /// Requests a new application instance (`core.spawn`).
    ///
    /// `path` and `args` are the bytes of the two `string` buffers. `diag` is a
    /// slice shaped like the `Diagnostic` record ([`layout::diagnostic::SIZE`]
    /// bytes): the host fills it on a synchronous failure and clears it on
    /// success, at the offsets [`layout::diagnostic`] fixes. A successful call
    /// returns the reserved global task id; an error is a `Status` with no task
    /// created.
    fn spawn(&mut self, _path: &[u8], _args: &[u8], _diag: &mut [u8]) -> Result<u64, Status> {
        Err(Status::Invalid)
    }

    /// Consumes the exit record for `id`, or reports that the child is live
    /// (`core.wait`). A live child makes the machine return
    /// [`Outcome::Blocked`] and retry the same instruction when the scheduler
    /// wakes the task.
    fn wait(&mut self, _id: u64) -> WaitOutcome {
        WaitOutcome::Status(Status::Invalid)
    }

    /// Requests termination of `id` (`core.kill`). It never waits.
    fn kill(&mut self, _id: u64) -> Status {
        Status::Invalid
    }

    /// Copies the request-completion diagnostic for `id`, if the host kept one.
    ///
    /// A failure the REPL service discovers *after* `spawn` returned (a compile
    /// error) is not known to `spawn`; the exit record carries the compiler's
    /// `Diagnostic` image and the kernel hands it over here. `diag` is shaped
    /// like the record ([`layout::diagnostic::SIZE`] bytes). Returns `false`
    /// when no diagnostic is available, in which case `diag` is untouched.
    /// `core.run` is the only caller; `core.wait` keeps its spec signature and
    /// observes only `Faulted(IoError)`.
    fn diagnostic_for(&mut self, _id: u64, _diag: &mut [u8]) -> bool {
        false
    }
}

/// What a blocked builtin is waiting for. The scheduler records the condition
/// on the task and retries the instruction after the wake-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    /// Wait for the task with this global id to terminate.
    ChildExit(u64),
    /// Wait for one finished console line (`core.read_line`).
    Console,
}

/// Result of [`Host::wait`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    /// The child has terminated; the record was consumed.
    Exited(ExitReason),
    /// The child is live. The machine blocks without advancing.
    Blocked,
    /// The id is unknown or its record was evicted.
    Status(Status),
}

/// Result of one [`Machine::run`] slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The step budget ran out (or a yield was requested); call `run` again to
    /// resume at the same instruction boundary.
    Yielded,
    /// A blocking builtin cannot complete. The machine has not advanced `pc`
    /// or the operand stack, so the same instruction is retried on wake.
    Blocked(WaitKind),
    /// The task stopped. No further execution is possible.
    Exited(ExitReason),
}

/// What one instruction did.
enum Step {
    /// Continue with the next instruction.
    Continue,
    /// The entry function returned; the task is done.
    Exited(ExitReason),
    /// A builtin blocked; the caller restores the instruction boundary.
    Blocked(WaitKind),
}

const FRAME_HEADER: usize = 32;
const NO_PREVIOUS_FRAME: u64 = u64::MAX;

/// A suspended or running bytecode task.
#[derive(Debug)]
pub struct Machine {
    started: bool,
    halted: Option<ExitReason>,
    fp: usize,
    sp: usize,
    pc: usize,
    func: u32,
    operand_base: usize,
    operand_off: usize,
    frame_size: usize,
    yield_requested: bool,
    executed: u64,
    /// The child id of a `core.run` that has already spawned and is waiting.
    ///
    /// Kept in the machine rather than the host so a `Blocked` retry resumes
    /// the wait without spawning a second child, and so the host stays
    /// stateless across scheduler quanta.
    pending_run: Option<u64>,
}

impl Default for Machine {
    fn default() -> Self {
        Machine::new()
    }
}

impl Machine {
    /// Creates a machine that has not started yet.
    pub const fn new() -> Self {
        Machine {
            started: false,
            halted: None,
            fp: 0,
            sp: 0,
            pc: 0,
            func: 0,
            operand_base: 0,
            operand_off: 0,
            frame_size: 0,
            yield_requested: false,
            executed: 0,
            pending_run: None,
        }
    }

    /// Returns the exit reason if the machine has stopped.
    pub fn halted(&self) -> Option<ExitReason> {
        self.halted
    }

    /// Total bytecodes executed since the machine was created.
    ///
    /// The scheduler uses this for the hard step cap of the total CPU budget:
    /// the 1 ms clock cannot resolve a sub-millisecond quantum, so time alone
    /// under-counts (spec 0007, "Budget").
    pub fn steps(&self) -> u64 {
        self.executed
    }

    /// Asks the machine to return [`Outcome::Yielded`] at the next instruction
    /// boundary. The request is cleared when it is honoured.
    pub fn request_yield(&mut self) {
        self.yield_requested = true;
    }

    /// Byte offset of the next instruction in the instruction section.
    pub fn pc(&self) -> u32 {
        self.pc as u32
    }

    /// Starts the entry function in a fresh frame.
    fn start(&mut self, program: &Program, stack: &mut [u8], args: &[u8]) -> Result<(), FaultCode> {
        let entry = program.header().entry;
        let function = program.function(entry).ok_or(FaultCode::IllegalOperation)?;
        let frame_size = function.frame_size as usize;
        let operand_off = function.operand_off as usize;
        if operand_off < FRAME_HEADER + function.param_count as usize * 8 {
            return Err(FaultCode::IllegalOperation);
        }
        if frame_size < operand_off || frame_size > stack.len() {
            return Err(FaultCode::StackOverflow);
        }
        stack[..frame_size].fill(0);
        write_u64(stack, 0, NO_PREVIOUS_FRAME)?;
        write_u32(stack, 8, 0)?;
        write_u32(stack, 12, u32::MAX)?;
        write_u64(stack, 16, 0)?;
        write_u64(stack, 24, 0)?;
        if function.param_count >= 1 {
            // `main(args: const string[1024])` receives the loader's buffer,
            // which is region 3 at data offset 8.
            if args.len() < 8 {
                return Err(FaultCode::OutOfBounds);
            }
            write_u64(stack, FRAME_HEADER, refs::encode(refs::REGION_ARGS, 8))?;
        }
        self.fp = 0;
        self.sp = operand_off;
        self.pc = function.code_off as usize;
        self.func = entry;
        self.operand_base = operand_off;
        self.operand_off = operand_off;
        self.frame_size = frame_size;
        self.started = true;
        self.halted = None;
        Ok(())
    }

    /// Executes at most `budget` instructions.
    ///
    /// `state`, `stack` and `args` must be the same slices on every resume.
    /// A structural problem that the verifier could not see becomes a
    /// [`FaultCode`], never a panic.
    pub fn run<H: Host>(
        &mut self,
        program: &Program,
        host: &mut H,
        state: &mut [u8],
        stack: &mut [u8],
        args: &[u8],
        budget: u64,
    ) -> Outcome {
        if let Some(reason) = self.halted {
            return Outcome::Exited(reason);
        }
        if !self.started {
            if let Err(code) = self.start(program, stack, args) {
                let reason = ExitReason::Faulted(code);
                self.halted = Some(reason);
                return Outcome::Exited(reason);
            }
        }

        let mut steps = budget;
        loop {
            if steps == 0 {
                return Outcome::Yielded;
            }
            if self.yield_requested {
                self.yield_requested = false;
                return Outcome::Yielded;
            }
            steps -= 1;
            self.executed = self.executed.wrapping_add(1);
            match self.step(program, host, state, stack, args) {
                Ok(Step::Exited(reason)) => {
                    self.halted = Some(reason);
                    return Outcome::Exited(reason);
                }
                Ok(Step::Blocked(kind)) => return Outcome::Blocked(kind),
                Ok(Step::Continue) => {}
                Err(code) => {
                    let reason = ExitReason::Faulted(code);
                    self.halted = Some(reason);
                    return Outcome::Exited(reason);
                }
            }
        }
    }

    fn step<H: Host>(
        &mut self,
        program: &Program,
        host: &mut H,
        state: &mut [u8],
        stack: &mut [u8],
        args: &[u8],
    ) -> Result<Step, FaultCode> {
        let code = program.instructions();
        let constants = program.constants();
        let pc = self.pc;
        let saved_sp = self.sp;
        if pc.checked_add(4).is_none_or(|end| end > code.len()) {
            return Err(FaultCode::IllegalOperation);
        }
        let op = code[pc];
        let a = code[pc + 1];
        let b = code[pc + 2];
        let c = code[pc + 3];
        let words = opcodes::instruction_words(op).ok_or(FaultCode::IllegalOperation)? as usize;
        let next = pc
            .checked_add(words.checked_mul(4).ok_or(FaultCode::IllegalOperation)?)
            .ok_or(FaultCode::IllegalOperation)?;
        if next > code.len() {
            return Err(FaultCode::IllegalOperation);
        }
        self.pc = next;

        match op {
            opcodes::DROP => {
                self.pop(stack)?;
            }
            opcodes::CONST => {
                let low = read_u32(code, pc + 4)? as u64;
                let high = read_u32(code, pc + 8)? as u64;
                self.push(stack, low | (high << 32))?;
            }
            opcodes::JMP => {
                self.pc = jump_target(next, code, pc)?;
            }
            opcodes::JZ => {
                let value = self.pop(stack)?;
                if value == 0 {
                    self.pc = jump_target(next, code, pc)?;
                }
            }
            opcodes::JNZ => {
                let value = self.pop(stack)?;
                if value != 0 {
                    self.pc = jump_target(next, code, pc)?;
                }
            }
            opcodes::CALL => {
                let index = read_u32(code, pc + 4)?;
                self.call(program, stack, index)?;
            }
            opcodes::RET => {
                if let Some(reason) = self.ret(program, state, stack, args)? {
                    return Ok(Step::Exited(reason));
                }
            }

            opcodes::LOAD_LOCAL => {
                let offset = read_u32(code, pc + 4)?;
                let address = self.local_address(offset, stack.len())?;
                let value = read_u64(stack, address)?;
                self.push(stack, value)?;
            }
            opcodes::STORE_LOCAL => {
                let offset = read_u32(code, pc + 4)?;
                let address = self.local_address(offset, stack.len())?;
                let value = self.pop(stack)?;
                write_u64(stack, address, value)?;
            }
            opcodes::REF_LOCAL => {
                let offset = read_u32(code, pc + 4)?;
                let address = self.local_address(offset, stack.len())?;
                self.push(stack, refs::encode(refs::REGION_STACK, address as u64))?;
            }
            opcodes::LOAD_STATE => {
                let offset = read_u32(code, pc + 4)? as usize;
                let value = read_u64(state, offset)?;
                self.push(stack, value)?;
            }
            opcodes::STORE_STATE => {
                let offset = read_u32(code, pc + 4)? as usize;
                let value = self.pop(stack)?;
                write_u64(state, offset, value)?;
            }
            opcodes::REF_STATE => {
                let offset = read_u32(code, pc + 4)? as usize;
                if offset.checked_add(8).is_none_or(|end| end > state.len()) {
                    return Err(FaultCode::OutOfBounds);
                }
                self.push(stack, refs::encode(refs::REGION_STATE, offset as u64))?;
            }
            opcodes::LOAD_REF => {
                let bits = self.pop(stack)?;
                let (region, offset) = self.resolve(bits, state, stack, constants, args)?;
                let value = match region {
                    refs::REGION_STATE => read_u64(state, offset)?,
                    refs::REGION_STACK => read_u64(stack, offset)?,
                    refs::REGION_CODE => read_u64(constants, offset)?,
                    refs::REGION_ARGS => read_u64(args, offset)?,
                    _ => return Err(FaultCode::OutOfBounds),
                };
                self.push(stack, value)?;
            }
            opcodes::STORE_REF => {
                let value = self.pop(stack)?;
                let bits = self.pop(stack)?;
                let (region, offset) = self.resolve(bits, state, stack, constants, args)?;
                match region {
                    refs::REGION_STATE => write_u64(state, offset, value)?,
                    refs::REGION_STACK => write_u64(stack, offset, value)?,
                    _ => return Err(FaultCode::IllegalOperation),
                }
            }
            opcodes::REF_CONST => {
                let offset = read_u32(code, pc + 4)? as usize;
                self.push(stack, refs::encode(refs::REGION_CODE, offset as u64))?;
            }
            opcodes::LOAD_U8 => {
                let index = self.pop(stack)?;
                let bits = self.pop(stack)?;
                let (region, offset) = self.resolve(bits, state, stack, constants, args)?;
                // A read is checked against the runtime length, so the byte is
                // always one the buffer actually holds (spec 0008, B4).
                let length = self.buffer_len(region, offset, state, stack, constants, args)?;
                let index = usize::try_from(index).map_err(|_| FaultCode::OutOfBounds)?;
                if index >= length {
                    return Err(FaultCode::OutOfBounds);
                }
                let address = offset.checked_add(index).ok_or(FaultCode::OutOfBounds)?;
                let value = byte_at(region, address, state, stack, constants, args)?;
                self.push(stack, u64::from(value))?;
            }
            opcodes::STORE_U8 => {
                let capacity = read_u32(code, pc + 4)? as usize;
                let value = self.pop(stack)?;
                let index = self.pop(stack)?;
                let bits = self.pop(stack)?;
                if value > u8::MAX as u64 {
                    return Err(FaultCode::Overflow);
                }
                let (region, offset) = self.resolve(bits, state, stack, constants, args)?;
                if region != refs::REGION_STATE && region != refs::REGION_STACK {
                    return Err(FaultCode::IllegalOperation);
                }
                if offset < 8 {
                    return Err(FaultCode::OutOfBounds);
                }
                let end = region_len(region, state, stack, constants, args)
                    .ok_or(FaultCode::OutOfBounds)?;
                let limit = offset.checked_add(capacity).ok_or(FaultCode::OutOfBounds)?;
                if limit > end {
                    return Err(FaultCode::OutOfBounds);
                }
                // A write is checked against the declared capacity, which is
                // the region the compiler proved available.
                let index = usize::try_from(index).map_err(|_| FaultCode::CapacityExceeded)?;
                if index >= capacity {
                    return Err(FaultCode::CapacityExceeded);
                }
                let length = self.buffer_len(region, offset, state, stack, constants, args)?;
                if length > capacity {
                    return Err(FaultCode::CapacityExceeded);
                }
                let address = offset + index;
                // Writing at or past the length extends it; the gap keeps no
                // stale bytes, so it is zero-filled before the length moves.
                let extended = index >= length;
                if extended && index > length {
                    match region {
                        refs::REGION_STATE => state[offset + length..address].fill(0),
                        refs::REGION_STACK => stack[offset + length..address].fill(0),
                        _ => return Err(FaultCode::IllegalOperation),
                    }
                }
                match region {
                    refs::REGION_STATE => state[address] = value as u8,
                    refs::REGION_STACK => stack[address] = value as u8,
                    _ => return Err(FaultCode::IllegalOperation),
                }
                if extended {
                    let new_len = (index + 1) as u64;
                    match region {
                        refs::REGION_STATE => write_u64(state, offset - 8, new_len)?,
                        refs::REGION_STACK => write_u64(stack, offset - 8, new_len)?,
                        _ => return Err(FaultCode::IllegalOperation),
                    }
                }
            }
            opcodes::REF_OFFSET => {
                let offset = read_u32(code, pc + 4)? as u64;
                let bits = self.pop(stack)?;
                let (region, base) = self.resolve(bits, state, stack, constants, args)?;
                let length = region_len(region, state, stack, constants, args)
                    .ok_or(FaultCode::OutOfBounds)?;
                let base = u64::try_from(base).map_err(|_| FaultCode::OutOfBounds)?;
                let shifted = base.checked_add(offset).ok_or(FaultCode::OutOfBounds)?;
                let shifted = usize::try_from(shifted).map_err(|_| FaultCode::OutOfBounds)?;
                // A field reference must name at least one full 8-byte slot;
                // a wider field (a buffer or a record) checks the rest itself.
                if shifted.checked_add(8).is_none_or(|tail| tail > length) {
                    return Err(FaultCode::OutOfBounds);
                }
                self.push(stack, refs::encode(region, shifted as u64))?;
            }
            opcodes::REF_ELEMENT => {
                let stride = read_u32(code, pc + 4)? as u64;
                let count = read_u32(code, pc + 8)? as u64;
                let index = self.pop(stack)?;
                let bits = self.pop(stack)?;
                let (region, base) = self.resolve(bits, state, stack, constants, args)?;
                let length = region_len(region, state, stack, constants, args)
                    .ok_or(FaultCode::OutOfBounds)?;
                // A negative `int` index is a huge `uint` and fails here; an
                // index at or past the count is never clamped (spec 0003).
                if index >= count {
                    return Err(FaultCode::OutOfBounds);
                }
                let shift = index.checked_mul(stride).ok_or(FaultCode::OutOfBounds)?;
                let base = u64::try_from(base).map_err(|_| FaultCode::OutOfBounds)?;
                let shifted = base.checked_add(shift).ok_or(FaultCode::OutOfBounds)?;
                let shifted = usize::try_from(shifted).map_err(|_| FaultCode::OutOfBounds)?;
                // The whole element must fit, not just its first slot.
                let stride = usize::try_from(stride).map_err(|_| FaultCode::OutOfBounds)?;
                if shifted.checked_add(stride).is_none_or(|tail| tail > length) {
                    return Err(FaultCode::OutOfBounds);
                }
                self.push(stack, refs::encode(region, shifted as u64))?;
            }
            opcodes::COPY_REFS => {
                let count = self.pop(stack)?;
                let source_bits = self.pop(stack)?;
                let destination_bits = self.pop(stack)?;
                self.copy_refs(
                    destination_bits,
                    source_bits,
                    count,
                    state,
                    stack,
                    constants,
                    args,
                )?;
            }
            opcodes::CMP_BUFFERS => {
                let right_bits = self.pop(stack)?;
                let left_bits = self.pop(stack)?;
                let equal = {
                    let (left_region, left) =
                        self.resolve(left_bits, state, stack, constants, args)?;
                    let (right_region, right) =
                        self.resolve(right_bits, state, stack, constants, args)?;
                    let left_len =
                        self.buffer_len(left_region, left, state, stack, constants, args)?;
                    let right_len =
                        self.buffer_len(right_region, right, state, stack, constants, args)?;
                    if left_len != right_len {
                        false
                    } else {
                        let a = buffer_slice(
                            left_region,
                            left,
                            left_len,
                            state,
                            stack,
                            constants,
                            args,
                        )?;
                        let b = buffer_slice(
                            right_region,
                            right,
                            right_len,
                            state,
                            stack,
                            constants,
                            args,
                        )?;
                        a == b
                    }
                };
                self.push_bool(stack, equal)?;
            }

            opcodes::ADD => self.signed(stack, |a, b| a.checked_add(b))?,
            opcodes::SUB => self.signed(stack, |a, b| a.checked_sub(b))?,
            opcodes::MUL => self.signed(stack, |a, b| a.checked_mul(b))?,
            opcodes::DIV => self.divide(stack, true, false)?,
            opcodes::MOD => self.divide(stack, true, true)?,
            opcodes::NEG => {
                let value = self.pop(stack)? as i64;
                let result = value.checked_neg().ok_or(FaultCode::Overflow)?;
                self.push(stack, result as u64)?;
            }
            opcodes::ADD_U => self.unsigned(stack, |a, b| a.checked_add(b))?,
            opcodes::SUB_U => self.unsigned(stack, |a, b| a.checked_sub(b))?,
            opcodes::MUL_U => self.unsigned(stack, |a, b| a.checked_mul(b))?,
            opcodes::DIV_U => self.divide(stack, false, false)?,
            opcodes::MOD_U => self.divide(stack, false, true)?,

            opcodes::EQ => self.compare_signed(stack, |a, b| a == b)?,
            opcodes::NE => self.compare_signed(stack, |a, b| a != b)?,
            opcodes::LT => self.compare_signed(stack, |a, b| a < b)?,
            opcodes::LE => self.compare_signed(stack, |a, b| a <= b)?,
            opcodes::GT => self.compare_signed(stack, |a, b| a > b)?,
            opcodes::GE => self.compare_signed(stack, |a, b| a >= b)?,
            opcodes::LT_U => self.compare_unsigned(stack, |a, b| a < b)?,
            opcodes::LE_U => self.compare_unsigned(stack, |a, b| a <= b)?,
            opcodes::GT_U => self.compare_unsigned(stack, |a, b| a > b)?,
            opcodes::GE_U => self.compare_unsigned(stack, |a, b| a >= b)?,
            opcodes::NOT => {
                let value = self.pop(stack)?;
                self.push_bool(stack, value == 0)?;
            }

            opcodes::CLEAR => {
                let bits = self.pop(stack)?;
                let (region, offset) = self.resolve(bits, state, stack, constants, args)?;
                if region != refs::REGION_STATE && region != refs::REGION_STACK {
                    return Err(FaultCode::IllegalOperation);
                }
                if offset < 8 {
                    return Err(FaultCode::OutOfBounds);
                }
                match region {
                    refs::REGION_STATE => write_u64(state, offset - 8, 0)?,
                    refs::REGION_STACK => write_u64(stack, offset - 8, 0)?,
                    _ => return Err(FaultCode::IllegalOperation),
                }
            }
            opcodes::APPEND => {
                let capacity = read_u32(code, pc + 4)? as usize;
                self.append(capacity, state, stack, constants, args)?;
            }
            opcodes::READ_LINE => {
                let capacity = read_u32(code, pc + 4)? as usize;
                let bits = self.pop(stack)?;
                let (region, offset) = self.resolve(bits, state, stack, constants, args)?;
                if region != refs::REGION_STATE && region != refs::REGION_STACK {
                    return Err(FaultCode::IllegalOperation);
                }
                if offset < 8 {
                    return Err(FaultCode::OutOfBounds);
                }
                let end = offset.checked_add(capacity).ok_or(FaultCode::OutOfBounds)?;
                if end
                    > region_len(region, state, stack, constants, args)
                        .ok_or(FaultCode::OutOfBounds)?
                {
                    return Err(FaultCode::OutOfBounds);
                }
                let copied = match region {
                    refs::REGION_STATE => host.read_line(&mut state[offset..end]),
                    refs::REGION_STACK => host.read_line(&mut stack[offset..end]),
                    _ => return Err(FaultCode::IllegalOperation),
                };
                let Some(copied) = copied else {
                    // As with `wait`: restore the operand stack and point at
                    // the same instruction so the retry sees an unchanged
                    // boundary. Nothing was written to the destination.
                    self.pc = pc;
                    self.sp = saved_sp;
                    return Ok(Step::Blocked(WaitKind::Console));
                };
                let copied = copied.min(capacity);
                match region {
                    refs::REGION_STATE => write_u64(state, offset - 8, copied as u64)?,
                    refs::REGION_STACK => write_u64(stack, offset - 8, copied as u64)?,
                    _ => return Err(FaultCode::IllegalOperation),
                }
                self.push(stack, copied as u64)?;
            }
            opcodes::FIND => {
                let value = self.pop(stack)?;
                let bits = self.pop(stack)?;
                if value > u8::MAX as u64 {
                    return Err(FaultCode::Overflow);
                }
                let (region, offset) = self.resolve(bits, state, stack, constants, args)?;
                let length = self.buffer_len(region, offset, state, stack, constants, args)?;
                let end = offset.checked_add(length).ok_or(FaultCode::OutOfBounds)?;
                if end
                    > region_len(region, state, stack, constants, args)
                        .ok_or(FaultCode::OutOfBounds)?
                {
                    return Err(FaultCode::OutOfBounds);
                }
                // A byte scan over `0..length`; `string` is not special here
                // (spec 0003: `find` is a shared byte operation).
                let bytes = buffer_slice(region, offset, length, state, stack, constants, args)?;
                let found = match bytes.iter().position(|byte| *byte == value as u8) {
                    Some(index) => index as u64,
                    None => (-1i64) as u64,
                };
                self.push(stack, found)?;
            }
            opcodes::SUBSTR => {
                let kind = a;
                let capacity = read_u32(code, pc + 4)? as usize;
                self.substr(kind, capacity, state, stack, constants, args)?;
            }
            opcodes::FORMAT_INT => {
                let kind = a;
                let capacity = read_u32(code, pc + 4)? as usize;
                self.format_int(kind, capacity, state, stack, constants, args)?;
            }
            opcodes::ENUM_CAST => {
                // `EnumName(int)` construction: the ordinal must name a variant.
                // Anything else faults `Overflow`, so an invalid enum value
                // cannot exist (spec 0003, "Enums").
                let count = read_u32(code, pc + 4)? as u64;
                let value = self.pop(stack)?;
                if value >= count {
                    return Err(FaultCode::Overflow);
                }
                self.push(stack, value)?;
            }
            opcodes::BUILTIN => {
                let id = read_u32(code, pc + 4)?;
                match id {
                    builtin::WRITE => {
                        let bits = self.pop(stack)?;
                        let (region, offset) = self.resolve(bits, state, stack, constants, args)?;
                        let length =
                            self.buffer_len(region, offset, state, stack, constants, args)?;
                        let end = offset.checked_add(length).ok_or(FaultCode::OutOfBounds)?;
                        let region_len = region_len(region, state, stack, constants, args)
                            .ok_or(FaultCode::OutOfBounds)?;
                        if end > region_len {
                            return Err(FaultCode::OutOfBounds);
                        }
                        match region {
                            refs::REGION_STATE => host.console_write(&state[offset..end]),
                            refs::REGION_STACK => host.console_write(&stack[offset..end]),
                            refs::REGION_CODE => host.console_write(&constants[offset..end]),
                            refs::REGION_ARGS => host.console_write(&args[offset..end]),
                            _ => return Err(FaultCode::OutOfBounds),
                        }
                    }
                    builtin::CLOCK => {
                        let now = host.clock();
                        self.push(stack, now)?;
                    }
                    builtin::SPAWN => {
                        // `core.spawn(path, args, var diag, var id) -> Status`:
                        // the id reference is on top, then diag, args and path.
                        let id_bits = self.pop(stack)?;
                        let diag_bits = self.pop(stack)?;
                        let args_bits = self.pop(stack)?;
                        let path_bits = self.pop(stack)?;
                        // The host writes the diagnostic into a local slice
                        // shaped like the record, so the path/args borrows of
                        // `state`/`stack` and the destination record can never
                        // alias. The bytes are copied out afterwards.
                        let mut diag = [0u8; layout::diagnostic::SIZE as usize];
                        let result = {
                            let (path_region, path_offset) =
                                self.resolve(path_bits, state, stack, constants, args)?;
                            let (args_region, args_offset) =
                                self.resolve(args_bits, state, stack, constants, args)?;
                            let path_len = self.buffer_len(
                                path_region,
                                path_offset,
                                state,
                                stack,
                                constants,
                                args,
                            )?;
                            let args_len = self.buffer_len(
                                args_region,
                                args_offset,
                                state,
                                stack,
                                constants,
                                args,
                            )?;
                            let path = buffer_slice(
                                path_region,
                                path_offset,
                                path_len,
                                state,
                                stack,
                                constants,
                                args,
                            )?;
                            let argument = buffer_slice(
                                args_region,
                                args_offset,
                                args_len,
                                state,
                                stack,
                                constants,
                                args,
                            )?;
                            host.spawn(path, argument, &mut diag)
                        };
                        self.write_bytes(diag_bits, &diag, state, stack, constants, args)?;
                        match result {
                            Ok(id) => {
                                self.write_ref(id_bits, id, state, stack, constants, args)?;
                                self.push(stack, Status::Ok as u64)?;
                            }
                            Err(status) => {
                                // The id out-parameter is untouched on failure;
                                // `diag` was filled by the host.
                                self.push(stack, status as u64)?;
                            }
                        }
                    }
                    builtin::WAIT => {
                        // `core.wait(id, var reason) -> Status`: the reason
                        // reference is on top of the id.
                        let reason_bits = self.pop(stack)?;
                        let id = self.pop(stack)?;
                        match host.wait(id) {
                            WaitOutcome::Exited(reason) => {
                                let mut record = [0u8; layout::exit_reason::SIZE as usize];
                                layout::exit_reason::write(&mut record, reason);
                                self.write_bytes(
                                    reason_bits,
                                    &record,
                                    state,
                                    stack,
                                    constants,
                                    args,
                                )?;
                                self.push(stack, Status::Ok as u64)?;
                            }
                            WaitOutcome::Status(status) => {
                                self.push(stack, status as u64)?;
                            }
                            WaitOutcome::Blocked => {
                                // Put both operands back on the operand stack
                                // and point `pc` at the same instruction: the
                                // retry after the wake-up must see an unchanged
                                // boundary.
                                self.pc = pc;
                                self.sp = saved_sp;
                                return Ok(Step::Blocked(WaitKind::ChildExit(id)));
                            }
                        }
                    }
                    builtin::KILL => {
                        let id = self.pop(stack)?;
                        let status = host.kill(id);
                        self.push(stack, status as u64)?;
                    }
                    builtin::RUN => {
                        // `core.run(path, args, var diag, var reason) -> Status`:
                        // the reason reference is on top, then diag, args, path.
                        let reason_bits = self.pop(stack)?;
                        let diag_bits = self.pop(stack)?;
                        let args_bits = self.pop(stack)?;
                        let path_bits = self.pop(stack)?;

                        // The spawn result is staged exactly like `core.spawn`'s:
                        // the host fills a local record-shaped slice and the
                        // bytes are copied into the caller's record afterwards.
                        let mut diag = [0u8; layout::diagnostic::SIZE as usize];

                        let id = match self.pending_run {
                            // The wait was interrupted by a block: the child was
                            // already spawned, so never spawn again.
                            Some(id) => id,
                            None => {
                                let result = {
                                    let (path_region, path_offset) =
                                        self.resolve(path_bits, state, stack, constants, args)?;
                                    let (args_region, args_offset) =
                                        self.resolve(args_bits, state, stack, constants, args)?;
                                    let path_len = self.buffer_len(
                                        path_region,
                                        path_offset,
                                        state,
                                        stack,
                                        constants,
                                        args,
                                    )?;
                                    let args_len = self.buffer_len(
                                        args_region,
                                        args_offset,
                                        state,
                                        stack,
                                        constants,
                                        args,
                                    )?;
                                    let path = buffer_slice(
                                        path_region,
                                        path_offset,
                                        path_len,
                                        state,
                                        stack,
                                        constants,
                                        args,
                                    )?;
                                    let argument = buffer_slice(
                                        args_region,
                                        args_offset,
                                        args_len,
                                        state,
                                        stack,
                                        constants,
                                        args,
                                    )?;
                                    host.spawn(path, argument, &mut diag)
                                };
                                match result {
                                    Ok(id) => {
                                        // Success clears the diagnostic.
                                        self.write_bytes(
                                            diag_bits, &diag, state, stack, constants, args,
                                        )?;
                                        self.pending_run = Some(id);
                                        id
                                    }
                                    Err(status) => {
                                        // The reason out-parameter is untouched;
                                        // the host filled the diagnostic.
                                        self.write_bytes(
                                            diag_bits, &diag, state, stack, constants, args,
                                        )?;
                                        self.push(stack, status as u64)?;
                                        return Ok(Step::Continue);
                                    }
                                }
                            }
                        };

                        match host.wait(id) {
                            WaitOutcome::Exited(reason) => {
                                let mut record = [0u8; layout::exit_reason::SIZE as usize];
                                layout::exit_reason::write(&mut record, reason);
                                // A load failure the service found after the
                                // spawn returned is carried in the exit record;
                                // `run` is the only primitive that reports it.
                                let mut detail = [0u8; layout::diagnostic::SIZE as usize];
                                if host.diagnostic_for(id, &mut detail) {
                                    self.write_bytes(
                                        diag_bits, &detail, state, stack, constants, args,
                                    )?;
                                }
                                self.pending_run = None;
                                self.write_bytes(
                                    reason_bits,
                                    &record,
                                    state,
                                    stack,
                                    constants,
                                    args,
                                )?;
                                self.push(stack, Status::Ok as u64)?;
                            }
                            WaitOutcome::Status(status) => {
                                self.pending_run = None;
                                self.push(stack, status as u64)?;
                            }
                            WaitOutcome::Blocked => {
                                // As with `wait`: restore the operand stack and
                                // the instruction boundary. The pending id stays
                                // in the machine, so the retry waits only.
                                self.pc = pc;
                                self.sp = saved_sp;
                                return Ok(Step::Blocked(WaitKind::ChildExit(id)));
                            }
                        }
                    }
                    builtin::LEN => {
                        let bits = self.pop(stack)?;
                        let (region, offset) = self.resolve(bits, state, stack, constants, args)?;
                        let length =
                            self.buffer_len(region, offset, state, stack, constants, args)?;
                        let end = offset.checked_add(length).ok_or(FaultCode::OutOfBounds)?;
                        if end
                            > region_len(region, state, stack, constants, args)
                                .ok_or(FaultCode::OutOfBounds)?
                        {
                            return Err(FaultCode::OutOfBounds);
                        }
                        self.push(stack, length as u64)?;
                    }
                    builtin::PORT_IN8 => {
                        let port = self.pop(stack)?;
                        let value = host.port_in8(port as u16);
                        self.push(stack, u64::from(value))?;
                    }
                    builtin::PORT_OUT8 => {
                        let value = self.pop(stack)?;
                        let port = self.pop(stack)?;
                        if value > u8::MAX as u64 {
                            // `value > 255` is `Status::Invalid`; the port is
                            // not touched.
                            self.push(stack, status_code(Status::Invalid))?;
                        } else {
                            host.port_out8(port as u16, value as u8);
                            self.push(stack, 0)?;
                        }
                    }
                    _ => return Err(FaultCode::IllegalOperation),
                }
            }

            opcodes::FADD => self.float_binary(stack, |a, b| a + b)?,
            opcodes::FSUB => self.float_binary(stack, |a, b| a - b)?,
            opcodes::FMUL => self.float_binary(stack, |a, b| a * b)?,
            opcodes::FDIV => self.float_binary(stack, |a, b| a / b)?,
            opcodes::FNEG => {
                let value = f64::from_bits(self.pop(stack)?);
                self.push(stack, (-value).to_bits())?;
            }
            opcodes::FEQ => self.compare_float(stack, |a, b| a == b)?,
            opcodes::FNE => self.compare_float(stack, |a, b| a != b)?,
            opcodes::FLT => self.compare_float(stack, |a, b| a < b)?,
            opcodes::FLE => self.compare_float(stack, |a, b| a <= b)?,
            opcodes::FGT => self.compare_float(stack, |a, b| a > b)?,
            opcodes::FGE => self.compare_float(stack, |a, b| a >= b)?,

            opcodes::CONVERT => {
                let value = self.pop(stack)?;
                let converted = convert(a, b, value)?;
                self.push(stack, converted)?;
            }

            _ => return Err(FaultCode::IllegalOperation),
        }

        // `c` is read for the poll hint only; the first-milestone interpreter
        // checks the budget at every boundary, so no extra work is needed.
        let _ = c;
        Ok(Step::Continue)
    }

    // ---- operand stack ------------------------------------------------

    fn push(&mut self, stack: &mut [u8], value: u64) -> Result<(), FaultCode> {
        let end = self.sp.checked_add(8).ok_or(FaultCode::StackOverflow)?;
        let limit = self
            .fp
            .checked_add(self.frame_size)
            .ok_or(FaultCode::StackOverflow)?;
        if end > limit || end > stack.len() {
            return Err(FaultCode::StackOverflow);
        }
        write_u64(stack, self.sp, value)?;
        self.sp = end;
        Ok(())
    }

    fn pop(&mut self, stack: &[u8]) -> Result<u64, FaultCode> {
        let base = self
            .operand_base
            .checked_add(8)
            .ok_or(FaultCode::IllegalOperation)?;
        if self.sp < base {
            return Err(FaultCode::IllegalOperation);
        }
        let start = self.sp - 8;
        let value = read_u64(stack, start)?;
        self.sp = start;
        Ok(value)
    }

    fn push_bool(&mut self, stack: &mut [u8], value: bool) -> Result<(), FaultCode> {
        self.push(stack, u64::from(value))
    }

    fn pop_pair(&mut self, stack: &[u8]) -> Result<(u64, u64), FaultCode> {
        let right = self.pop(stack)?;
        let left = self.pop(stack)?;
        Ok((left, right))
    }

    fn signed(
        &mut self,
        stack: &mut [u8],
        operation: fn(i64, i64) -> Option<i64>,
    ) -> Result<(), FaultCode> {
        let (left, right) = self.pop_pair(stack)?;
        let result = operation(left as i64, right as i64).ok_or(FaultCode::Overflow)?;
        self.push(stack, result as u64)
    }

    fn unsigned(
        &mut self,
        stack: &mut [u8],
        operation: fn(u64, u64) -> Option<u64>,
    ) -> Result<(), FaultCode> {
        let (left, right) = self.pop_pair(stack)?;
        let result = operation(left, right).ok_or(FaultCode::Overflow)?;
        self.push(stack, result)
    }

    fn divide(&mut self, stack: &mut [u8], signed: bool, remainder: bool) -> Result<(), FaultCode> {
        let (left, right) = self.pop_pair(stack)?;
        if signed {
            let (left, right) = (left as i64, right as i64);
            if right == 0 {
                return Err(FaultCode::DivideByZero);
            }
            let result = if remainder {
                left.checked_rem(right)
            } else {
                left.checked_div(right)
            }
            .ok_or(FaultCode::Overflow)?;
            self.push(stack, result as u64)
        } else {
            if right == 0 {
                return Err(FaultCode::DivideByZero);
            }
            let result = if remainder {
                left.checked_rem(right)
            } else {
                left.checked_div(right)
            }
            .ok_or(FaultCode::Overflow)?;
            self.push(stack, result)
        }
    }

    fn compare_signed(
        &mut self,
        stack: &mut [u8],
        predicate: fn(i64, i64) -> bool,
    ) -> Result<(), FaultCode> {
        let (left, right) = self.pop_pair(stack)?;
        self.push_bool(stack, predicate(left as i64, right as i64))
    }

    fn compare_unsigned(
        &mut self,
        stack: &mut [u8],
        predicate: fn(u64, u64) -> bool,
    ) -> Result<(), FaultCode> {
        let (left, right) = self.pop_pair(stack)?;
        self.push_bool(stack, predicate(left, right))
    }

    fn compare_float(
        &mut self,
        stack: &mut [u8],
        predicate: fn(f64, f64) -> bool,
    ) -> Result<(), FaultCode> {
        let (left, right) = self.pop_pair(stack)?;
        self.push_bool(
            stack,
            predicate(f64::from_bits(left), f64::from_bits(right)),
        )
    }

    fn float_binary(
        &mut self,
        stack: &mut [u8],
        operation: fn(f64, f64) -> f64,
    ) -> Result<(), FaultCode> {
        let (left, right) = self.pop_pair(stack)?;
        self.push(
            stack,
            operation(f64::from_bits(left), f64::from_bits(right)).to_bits(),
        )
    }

    // ---- frames -------------------------------------------------------

    fn local_address(&self, offset: u32, stack_len: usize) -> Result<usize, FaultCode> {
        let address = self
            .fp
            .checked_add(FRAME_HEADER)
            .and_then(|base| base.checked_add(offset as usize))
            .ok_or(FaultCode::OutOfBounds)?;
        let end = address.checked_add(8).ok_or(FaultCode::OutOfBounds)?;
        let locals_end = self
            .fp
            .checked_add(self.operand_off)
            .ok_or(FaultCode::OutOfBounds)?;
        if end > locals_end || end > stack_len {
            return Err(FaultCode::OutOfBounds);
        }
        Ok(address)
    }

    fn call(&mut self, program: &Program, stack: &mut [u8], index: u32) -> Result<(), FaultCode> {
        let callee = program.function(index).ok_or(FaultCode::IllegalOperation)?;
        let argc = callee.param_count as usize;
        let args_bytes = argc.checked_mul(8).ok_or(FaultCode::IllegalOperation)?;
        let args_start = self
            .sp
            .checked_sub(args_bytes)
            .ok_or(FaultCode::IllegalOperation)?;
        if args_start < self.operand_base {
            return Err(FaultCode::IllegalOperation);
        }
        let new_fp = self
            .fp
            .checked_add(self.frame_size)
            .ok_or(FaultCode::StackOverflow)?;
        let new_end = new_fp
            .checked_add(callee.frame_size as usize)
            .ok_or(FaultCode::StackOverflow)?;
        if new_end > stack.len() {
            return Err(FaultCode::StackOverflow);
        }
        let dest_ref = if callee.has_destination {
            let base = args_start
                .checked_sub(8)
                .ok_or(FaultCode::IllegalOperation)?;
            if base < self.operand_base {
                return Err(FaultCode::IllegalOperation);
            }
            read_u64(stack, base)?
        } else {
            0
        };

        let copy_start = new_fp
            .checked_add(FRAME_HEADER)
            .ok_or(FaultCode::StackOverflow)?;
        if copy_start
            .checked_add(args_bytes)
            .is_none_or(|end| end > new_end)
        {
            return Err(FaultCode::IllegalOperation);
        }
        stack[new_fp..new_end].fill(0);
        stack.copy_within(args_start..self.sp, copy_start);

        write_u64(stack, new_fp, self.fp as u64)?;
        write_u32(stack, new_fp + 8, self.pc as u32)?;
        write_u32(stack, new_fp + 12, self.func)?;
        write_u64(stack, new_fp + 16, args_start as u64)?;
        write_u64(stack, new_fp + 24, dest_ref)?;

        self.fp = new_fp;
        self.func = index;
        self.operand_off = callee.operand_off as usize;
        self.frame_size = callee.frame_size as usize;
        self.operand_base = new_fp
            .checked_add(self.operand_off)
            .ok_or(FaultCode::StackOverflow)?;
        self.sp = self.operand_base;
        self.pc = callee.code_off as usize;
        Ok(())
    }

    fn ret(
        &mut self,
        program: &Program,
        state: &mut [u8],
        stack: &mut [u8],
        args: &[u8],
    ) -> Result<Option<ExitReason>, FaultCode> {
        let current = program
            .function(self.func)
            .ok_or(FaultCode::IllegalOperation)?;
        if current.has_destination {
            let value = self.pop(stack)?;
            let destination = read_u64(stack, self.fp + 24)?;
            let (region, offset) =
                self.resolve(destination, state, stack, program.constants(), args)?;
            match region {
                refs::REGION_STATE => write_u64(state, offset, value)?,
                refs::REGION_STACK => write_u64(stack, offset, value)?,
                _ => return Err(FaultCode::IllegalOperation),
            }
        }

        // The frame header stores the *caller's* state, so all of it is read
        // from the current frame, not from the previous one.
        let previous = read_u64(stack, self.fp)?;
        if previous == NO_PREVIOUS_FRAME {
            return Ok(Some(ExitReason::Ok));
        }
        let return_pc = read_u32(stack, self.fp + 8)? as usize;
        let caller_func = read_u32(stack, self.fp + 12)?;
        let return_sp = read_u64(stack, self.fp + 16)?;
        let previous = usize::try_from(previous).map_err(|_| FaultCode::OutOfBounds)?;
        let caller = program
            .function(caller_func)
            .ok_or(FaultCode::IllegalOperation)?;
        self.fp = previous;
        self.pc = return_pc;
        self.func = caller_func;
        self.sp = usize::try_from(return_sp).map_err(|_| FaultCode::OutOfBounds)?;
        self.operand_off = caller.operand_off as usize;
        self.frame_size = caller.frame_size as usize;
        self.operand_base = previous
            .checked_add(self.operand_off)
            .ok_or(FaultCode::OutOfBounds)?;
        Ok(None)
    }

    // ---- references and buffers ---------------------------------------

    fn resolve(
        &self,
        bits: u64,
        state: &[u8],
        stack: &[u8],
        code: &[u8],
        args: &[u8],
    ) -> Result<(u8, usize), FaultCode> {
        let reference = refs::decode(bits);
        let length =
            region_len(reference.region, state, stack, code, args).ok_or(FaultCode::OutOfBounds)?;
        let offset = usize::try_from(reference.offset).map_err(|_| FaultCode::OutOfBounds)?;
        if offset > length {
            return Err(FaultCode::OutOfBounds);
        }
        Ok((reference.region, offset))
    }

    /// Writes one scalar through a `var` reference (`core.spawn`'s id and
    /// `core.wait`'s reason). Only `state` and `stack` are writable; a
    /// reference into code or the argument buffer is `IllegalOperation`.
    fn write_ref(
        &self,
        bits: u64,
        value: u64,
        state: &mut [u8],
        stack: &mut [u8],
        code: &[u8],
        args: &[u8],
    ) -> Result<(), FaultCode> {
        let (region, offset) = self.resolve(bits, state, stack, code, args)?;
        match region {
            refs::REGION_STATE => write_u64(state, offset, value),
            refs::REGION_STACK => write_u64(stack, offset, value),
            _ => Err(FaultCode::IllegalOperation),
        }
    }

    /// Copies a whole record image through a `var` reference (`core.spawn`'s
    /// `diag`, `core.wait`'s `reason`). The caller supplies the bytes, shaped by
    /// `abi::layout`; the destination must be a writable region wide enough for
    /// every byte.
    fn write_bytes(
        &self,
        bits: u64,
        bytes: &[u8],
        state: &mut [u8],
        stack: &mut [u8],
        code: &[u8],
        args: &[u8],
    ) -> Result<(), FaultCode> {
        let (region, offset) = self.resolve(bits, state, stack, code, args)?;
        if region != refs::REGION_STATE && region != refs::REGION_STACK {
            return Err(FaultCode::IllegalOperation);
        }
        let end = offset
            .checked_add(bytes.len())
            .ok_or(FaultCode::OutOfBounds)?;
        if end > region_len(region, state, stack, code, args).ok_or(FaultCode::OutOfBounds)? {
            return Err(FaultCode::OutOfBounds);
        }
        match region {
            refs::REGION_STATE => state[offset..end].copy_from_slice(bytes),
            refs::REGION_STACK => stack[offset..end].copy_from_slice(bytes),
            _ => return Err(FaultCode::IllegalOperation),
        }
        Ok(())
    }

    /// `copy_refs`: copy `count` bytes from one reference to another.
    ///
    /// Used for a whole-record copy. Both regions are bounds-checked, the
    /// destination must be writable, and a copy inside one region is a move so
    /// overlapping records cannot corrupt the source.
    fn copy_refs(
        &mut self,
        destination_bits: u64,
        source_bits: u64,
        count: u64,
        state: &mut [u8],
        stack: &mut [u8],
        code: &[u8],
        args: &[u8],
    ) -> Result<(), FaultCode> {
        let (destination_region, destination) =
            self.resolve(destination_bits, state, stack, code, args)?;
        let (source_region, source) = self.resolve(source_bits, state, stack, code, args)?;
        if destination_region != refs::REGION_STATE && destination_region != refs::REGION_STACK {
            return Err(FaultCode::IllegalOperation);
        }
        let count = usize::try_from(count).map_err(|_| FaultCode::OutOfBounds)?;
        let destination_end = destination
            .checked_add(count)
            .ok_or(FaultCode::OutOfBounds)?;
        let source_end = source.checked_add(count).ok_or(FaultCode::OutOfBounds)?;
        if destination_end
            > region_len(destination_region, state, stack, code, args)
                .ok_or(FaultCode::OutOfBounds)?
            || source_end
                > region_len(source_region, state, stack, code, args)
                    .ok_or(FaultCode::OutOfBounds)?
        {
            return Err(FaultCode::OutOfBounds);
        }
        if count == 0 {
            return Ok(());
        }
        if destination_region == source_region {
            match destination_region {
                refs::REGION_STATE => state.copy_within(source..source_end, destination),
                refs::REGION_STACK => stack.copy_within(source..source_end, destination),
                _ => return Err(FaultCode::IllegalOperation),
            }
        } else {
            match destination_region {
                refs::REGION_STATE => {
                    let source_bytes = match source_region {
                        refs::REGION_STACK => &stack[source..source_end],
                        refs::REGION_CODE => &code[source..source_end],
                        refs::REGION_ARGS => &args[source..source_end],
                        _ => return Err(FaultCode::OutOfBounds),
                    };
                    state[destination..destination_end].copy_from_slice(source_bytes);
                }
                refs::REGION_STACK => {
                    let source_bytes = match source_region {
                        refs::REGION_STATE => &state[source..source_end],
                        refs::REGION_CODE => &code[source..source_end],
                        refs::REGION_ARGS => &args[source..source_end],
                        _ => return Err(FaultCode::OutOfBounds),
                    };
                    stack[destination..destination_end].copy_from_slice(source_bytes);
                }
                _ => return Err(FaultCode::IllegalOperation),
            }
        }
        Ok(())
    }

    fn buffer_len(
        &self,
        region: u8,
        offset: usize,
        state: &[u8],
        stack: &[u8],
        code: &[u8],
        args: &[u8],
    ) -> Result<usize, FaultCode> {
        if offset < 8 {
            return Err(FaultCode::OutOfBounds);
        }
        let raw = match region {
            refs::REGION_STATE => read_u64(state, offset - 8)?,
            refs::REGION_STACK => read_u64(stack, offset - 8)?,
            refs::REGION_CODE => read_u64(code, offset - 8)?,
            refs::REGION_ARGS => read_u64(args, offset - 8)?,
            _ => return Err(FaultCode::OutOfBounds),
        };
        usize::try_from(raw).map_err(|_| FaultCode::OutOfBounds)
    }

    fn append(
        &mut self,
        capacity: usize,
        state: &mut [u8],
        stack: &mut [u8],
        code: &[u8],
        args: &[u8],
    ) -> Result<(), FaultCode> {
        let source_bits = self.pop(stack)?;
        let destination_bits = self.pop(stack)?;
        let (destination_region, destination) =
            self.resolve(destination_bits, state, stack, code, args)?;
        let (source_region, source) = self.resolve(source_bits, state, stack, code, args)?;

        if destination_region != refs::REGION_STATE && destination_region != refs::REGION_STACK {
            return Err(FaultCode::IllegalOperation);
        }
        if destination < 8 || source < 8 {
            return Err(FaultCode::OutOfBounds);
        }

        let destination_len =
            self.buffer_len(destination_region, destination, state, stack, code, args)?;
        let source_len = self.buffer_len(source_region, source, state, stack, code, args)?;

        let destination_limit = destination
            .checked_add(capacity)
            .ok_or(FaultCode::OutOfBounds)?;
        if destination_limit
            > region_len(destination_region, state, stack, code, args)
                .ok_or(FaultCode::OutOfBounds)?
        {
            return Err(FaultCode::OutOfBounds);
        }
        let source_end = source
            .checked_add(source_len)
            .ok_or(FaultCode::OutOfBounds)?;
        if source_end
            > region_len(source_region, state, stack, code, args).ok_or(FaultCode::OutOfBounds)?
        {
            return Err(FaultCode::OutOfBounds);
        }
        if destination_len > capacity {
            return Err(FaultCode::CapacityExceeded);
        }
        let new_len = destination_len
            .checked_add(source_len)
            .ok_or(FaultCode::CapacityExceeded)?;
        if new_len > capacity {
            return Err(FaultCode::CapacityExceeded);
        }
        if destination_region == source_region && destination == source {
            // The compiler rejects statically visible aliasing; this catches a
            // reference case or a corrupt package before it corrupts memory.
            return Err(FaultCode::IllegalOperation);
        }
        // Appends write after the current length, never over it.
        let copy_start = destination
            .checked_add(destination_len)
            .ok_or(FaultCode::OutOfBounds)?;
        let copy_end = copy_start
            .checked_add(source_len)
            .ok_or(FaultCode::OutOfBounds)?;

        if destination_region == source_region {
            match destination_region {
                refs::REGION_STATE => {
                    state.copy_within(source..source_end, copy_start);
                }
                refs::REGION_STACK => {
                    stack.copy_within(source..source_end, copy_start);
                }
                _ => return Err(FaultCode::IllegalOperation),
            }
        } else {
            match destination_region {
                refs::REGION_STATE => match source_region {
                    refs::REGION_STACK => {
                        state[copy_start..copy_end].copy_from_slice(&stack[source..source_end]);
                    }
                    refs::REGION_CODE => {
                        state[copy_start..copy_end].copy_from_slice(&code[source..source_end]);
                    }
                    refs::REGION_ARGS => {
                        state[copy_start..copy_end].copy_from_slice(&args[source..source_end]);
                    }
                    _ => return Err(FaultCode::OutOfBounds),
                },
                refs::REGION_STACK => match source_region {
                    refs::REGION_STATE => {
                        stack[copy_start..copy_end].copy_from_slice(&state[source..source_end]);
                    }
                    refs::REGION_CODE => {
                        stack[copy_start..copy_end].copy_from_slice(&code[source..source_end]);
                    }
                    refs::REGION_ARGS => {
                        stack[copy_start..copy_end].copy_from_slice(&args[source..source_end]);
                    }
                    _ => return Err(FaultCode::OutOfBounds),
                },
                _ => return Err(FaultCode::IllegalOperation),
            }
        }

        match destination_region {
            refs::REGION_STATE => write_u64(state, destination - 8, new_len as u64)?,
            refs::REGION_STACK => write_u64(stack, destination - 8, new_len as u64)?,
            _ => return Err(FaultCode::IllegalOperation),
        }
        Ok(())
    }

    /// `substr`: copy a slice of `source` into `destination` (`core.substr_into`).
    ///
    /// `kind` is [`opcodes::buffer_kind`]; for `STRING` both slice boundaries
    /// must fall on UTF-8 code-point boundaries in the source (spec 0003, T4).
    /// Negative offsets push `-4` (`Status::Invalid`). A start past the source
    /// length copies nothing. Nothing is written when the call is rejected.
    fn substr(
        &mut self,
        kind: u8,
        capacity: usize,
        state: &mut [u8],
        stack: &mut [u8],
        code: &[u8],
        args: &[u8],
    ) -> Result<(), FaultCode> {
        if kind != opcodes::buffer_kind::STRING && kind != opcodes::buffer_kind::BYTES {
            return Err(FaultCode::IllegalOperation);
        }
        let length = self.pop(stack)? as i64;
        let start = self.pop(stack)? as i64;
        let source_bits = self.pop(stack)?;
        let destination_bits = self.pop(stack)?;

        let (destination_region, destination) =
            self.resolve(destination_bits, state, stack, code, args)?;
        let (source_region, source) = self.resolve(source_bits, state, stack, code, args)?;
        if destination_region != refs::REGION_STATE && destination_region != refs::REGION_STACK {
            return Err(FaultCode::IllegalOperation);
        }
        if destination < 8 || source < 8 {
            return Err(FaultCode::OutOfBounds);
        }
        let destination_len =
            self.buffer_len(destination_region, destination, state, stack, code, args)?;
        let source_len = self.buffer_len(source_region, source, state, stack, code, args)?;

        // The declared destination capacity is the bound the compiler proved.
        let destination_end = destination
            .checked_add(capacity)
            .ok_or(FaultCode::OutOfBounds)?;
        if destination_end
            > region_len(destination_region, state, stack, code, args)
                .ok_or(FaultCode::OutOfBounds)?
        {
            return Err(FaultCode::OutOfBounds);
        }
        let source_end = source
            .checked_add(source_len)
            .ok_or(FaultCode::OutOfBounds)?;
        if source_end
            > region_len(source_region, state, stack, code, args).ok_or(FaultCode::OutOfBounds)?
        {
            return Err(FaultCode::OutOfBounds);
        }
        if destination_len > capacity {
            return Err(FaultCode::CapacityExceeded);
        }

        if start < 0 || length < 0 {
            self.push(stack, status_code(Status::Invalid))?;
            return Ok(());
        }
        let start = start as u64;
        let length = length as u64;
        let in_range = start <= source_len as u64;
        // `start > src_len` copies nothing; otherwise clamp to the source tail
        // and the destination capacity.
        let copied = if in_range {
            let available = source_len as u64 - start;
            usize::try_from(length.min(available).min(capacity as u64))
                .map_err(|_| FaultCode::OutOfBounds)?
        } else {
            0
        };

        if kind == opcodes::buffer_kind::STRING && in_range {
            let bytes = buffer_slice(source_region, source, source_len, state, stack, code, args)?;
            let start = start as usize;
            if !utf8_boundary(bytes, start) || !utf8_boundary(bytes, start + copied) {
                // A slice that would split a code point is not a `string`
                // (spec 0003, T4): reject it and leave the destination alone.
                self.push(stack, status_code(Status::Invalid))?;
                return Ok(());
            }
        }

        if copied > 0 {
            let start = start as usize;
            let source_start = source.checked_add(start).ok_or(FaultCode::OutOfBounds)?;
            let source_end = source_start
                .checked_add(copied)
                .ok_or(FaultCode::OutOfBounds)?;
            if destination_region == source_region {
                match destination_region {
                    refs::REGION_STATE => state.copy_within(source_start..source_end, destination),
                    refs::REGION_STACK => stack.copy_within(source_start..source_end, destination),
                    _ => return Err(FaultCode::IllegalOperation),
                }
            } else {
                match destination_region {
                    refs::REGION_STATE => match source_region {
                        refs::REGION_STACK => {
                            state[destination..destination + copied]
                                .copy_from_slice(&stack[source_start..source_end]);
                        }
                        refs::REGION_CODE => {
                            state[destination..destination + copied]
                                .copy_from_slice(&code[source_start..source_end]);
                        }
                        refs::REGION_ARGS => {
                            state[destination..destination + copied]
                                .copy_from_slice(&args[source_start..source_end]);
                        }
                        _ => return Err(FaultCode::OutOfBounds),
                    },
                    refs::REGION_STACK => match source_region {
                        refs::REGION_STATE => {
                            stack[destination..destination + copied]
                                .copy_from_slice(&state[source_start..source_end]);
                        }
                        refs::REGION_CODE => {
                            stack[destination..destination + copied]
                                .copy_from_slice(&code[source_start..source_end]);
                        }
                        refs::REGION_ARGS => {
                            stack[destination..destination + copied]
                                .copy_from_slice(&args[source_start..source_end]);
                        }
                        _ => return Err(FaultCode::OutOfBounds),
                    },
                    _ => return Err(FaultCode::IllegalOperation),
                }
            }
        }

        match destination_region {
            refs::REGION_STATE => write_u64(state, destination - 8, copied as u64)?,
            refs::REGION_STACK => write_u64(stack, destination - 8, copied as u64)?,
            _ => return Err(FaultCode::IllegalOperation),
        }
        self.push(stack, copied as u64)
    }

    /// `format_int`: write a signed integer as decimal or lowercase hex
    /// (`core.format_int_into`, spec 0003, "Standard library").
    ///
    /// `kind` is [`opcodes::buffer_kind`]; both kinds accept the ASCII output.
    /// `base` must be 10 or 16. A negative value gets a leading `-`, then the
    /// magnitude; `width` zero-pads the digits (the sign is extra). A result
    /// that does not fit `capacity` pushes `-6` (`Status::CapacityExceeded`)
    /// and leaves the destination length untouched; an unsupported base pushes
    /// `-4` (`Status::Invalid`) and also writes nothing.
    fn format_int(
        &mut self,
        kind: u8,
        capacity: usize,
        state: &mut [u8],
        stack: &mut [u8],
        code: &[u8],
        args: &[u8],
    ) -> Result<(), FaultCode> {
        if kind != opcodes::buffer_kind::STRING && kind != opcodes::buffer_kind::BYTES {
            return Err(FaultCode::IllegalOperation);
        }
        let width = self.pop(stack)?;
        let base = self.pop(stack)?;
        let value = self.pop(stack)? as i64;
        let destination_bits = self.pop(stack)?;

        let (destination_region, destination) =
            self.resolve(destination_bits, state, stack, code, args)?;
        if destination_region != refs::REGION_STATE && destination_region != refs::REGION_STACK {
            return Err(FaultCode::IllegalOperation);
        }
        if destination < 8 {
            return Err(FaultCode::OutOfBounds);
        }
        let destination_end = destination
            .checked_add(capacity)
            .ok_or(FaultCode::OutOfBounds)?;
        if destination_end
            > region_len(destination_region, state, stack, code, args)
                .ok_or(FaultCode::OutOfBounds)?
        {
            return Err(FaultCode::OutOfBounds);
        }
        if base != 10 && base != 16 {
            self.push(stack, status_code(Status::Invalid))?;
            return Ok(());
        }

        // Digits are produced least-significant first into a fixed array: a
        // `u64` needs at most 20 decimal digits, and the sign is separate. The
        // width is applied after the fact so a huge width costs no loop.
        let negative = value < 0;
        let mut rest = value.unsigned_abs();
        let mut digits = [0u8; 20];
        let mut cursor = digits.len();
        loop {
            cursor -= 1;
            let digit = (rest % base) as u8;
            digits[cursor] = if digit < 10 {
                b'0' + digit
            } else {
                b'a' + (digit - 10)
            };
            rest /= base;
            if rest == 0 {
                break;
            }
        }
        let written = digits.len() - cursor;
        let padded = (written as u64).max(width);
        let total = match padded.checked_add(u64::from(negative)) {
            Some(total) => total,
            None => {
                self.push(stack, status_code(Status::CapacityExceeded))?;
                return Ok(());
            }
        };
        if total > capacity as u64 {
            self.push(stack, status_code(Status::CapacityExceeded))?;
            return Ok(());
        }
        let total = total as usize;

        {
            let out = match destination_region {
                refs::REGION_STATE => &mut state[destination..destination + total],
                refs::REGION_STACK => &mut stack[destination..destination + total],
                _ => return Err(FaultCode::IllegalOperation),
            };
            let mut at = 0;
            if negative {
                out[at] = b'-';
                at += 1;
            }
            let pad = width.saturating_sub(written as u64) as usize;
            out[at..at + pad].fill(b'0');
            at += pad;
            out[at..at + written].copy_from_slice(&digits[cursor..]);
        }

        match destination_region {
            refs::REGION_STATE => write_u64(state, destination - 8, total as u64)?,
            refs::REGION_STACK => write_u64(stack, destination - 8, total as u64)?,
            _ => return Err(FaultCode::IllegalOperation),
        }
        self.push(stack, total as u64)
    }
}

/// True when `index` is a UTF-8 code-point boundary in `bytes`.
///
/// The source `string` invariant makes a boundary check sufficient: a
/// continuation byte (`10xxxxxx`) at the index means the slice would split a
/// sequence. `bytes.len()` is the end boundary.
fn utf8_boundary(bytes: &[u8], index: usize) -> bool {
    match bytes.get(index) {
        None => index == bytes.len(),
        Some(byte) => byte & 0xC0 != 0x80,
    }
}

/// Reads one byte from a region, bounds-checked like [`buffer_slice`].
fn byte_at(
    region: u8,
    offset: usize,
    state: &[u8],
    stack: &[u8],
    code: &[u8],
    args: &[u8],
) -> Result<u8, FaultCode> {
    let bytes = match region {
        refs::REGION_STATE => state,
        refs::REGION_STACK => stack,
        refs::REGION_CODE => code,
        refs::REGION_ARGS => args,
        _ => return Err(FaultCode::OutOfBounds),
    };
    bytes.get(offset).copied().ok_or(FaultCode::OutOfBounds)
}

/// Maps an [`abi::Status`] to the staged `int` result the buffer primitives
/// still use: the negated discriminant (`Invalid` is `-4`, `NotFound` is `-3`,
/// ...). `core.spawn`/`wait`/`kill` return the `Status` value itself, not this
/// encoding (spec 0005).
fn status_code(status: Status) -> u64 {
    (-(status as i64)) as u64
}

/// The bytes a buffer reference names, bounds-checked like `WRITE` does.
fn buffer_slice<'a>(
    region: u8,
    offset: usize,
    length: usize,
    state: &'a [u8],
    stack: &'a [u8],
    code: &'a [u8],
    args: &'a [u8],
) -> Result<&'a [u8], FaultCode> {
    let end = offset.checked_add(length).ok_or(FaultCode::OutOfBounds)?;
    let bytes = match region {
        refs::REGION_STATE => state,
        refs::REGION_STACK => stack,
        refs::REGION_CODE => code,
        refs::REGION_ARGS => args,
        _ => return Err(FaultCode::OutOfBounds),
    };
    bytes.get(offset..end).ok_or(FaultCode::OutOfBounds)
}

fn region_len(region: u8, state: &[u8], stack: &[u8], code: &[u8], args: &[u8]) -> Option<usize> {
    match region {
        refs::REGION_STATE => Some(state.len()),
        refs::REGION_STACK => Some(stack.len()),
        refs::REGION_CODE => Some(code.len()),
        refs::REGION_ARGS => Some(args.len()),
        _ => None,
    }
}

fn jump_target(next: usize, code: &[u8], pc: usize) -> Result<usize, FaultCode> {
    let relative = read_u32(code, pc + 4)? as i32;
    let target = next as i64 + relative as i64 * 4;
    if target < 0 {
        return Err(FaultCode::IllegalOperation);
    }
    let target = target as usize;
    if target >= code.len() {
        return Err(FaultCode::IllegalOperation);
    }
    Ok(target)
}

fn convert(from: u8, to: u8, bits: u64) -> Result<u64, FaultCode> {
    use value_kind::{BYTE, F64, INT, UINT};

    if from == to {
        return Ok(bits);
    }
    match (from, to) {
        (INT, UINT) => u64::try_from(bits as i64).map_err(|_| FaultCode::Overflow),
        (INT, BYTE) => {
            let value = bits as i64;
            if (0..=255).contains(&value) {
                Ok(value as u64)
            } else {
                Err(FaultCode::Overflow)
            }
        }
        (INT, F64) => Ok(((bits as i64) as f64).to_bits()),
        (UINT, INT) => i64::try_from(bits)
            .map(|value| value as u64)
            .map_err(|_| FaultCode::Overflow),
        (UINT, BYTE) => {
            if bits <= 255 {
                Ok(bits)
            } else {
                Err(FaultCode::Overflow)
            }
        }
        (UINT, F64) => Ok((bits as f64).to_bits()),
        (BYTE, INT) | (BYTE, UINT) => {
            if bits <= 255 {
                Ok(bits)
            } else {
                Err(FaultCode::Overflow)
            }
        }
        (BYTE, F64) => {
            if bits <= 255 {
                Ok((bits as f64).to_bits())
            } else {
                Err(FaultCode::Overflow)
            }
        }
        (F64, INT) => {
            // `f64` has no `trunc` in core; Rust's integer cast already
            // truncates toward zero, so only the range check is needed. Both
            // bounds are exact powers of two. NaN fails the comparison.
            let value = f64::from_bits(bits);
            if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&value) {
                Ok((value as i64) as u64)
            } else {
                Err(FaultCode::Overflow)
            }
        }
        (F64, UINT) => {
            let value = f64::from_bits(bits);
            if (0.0..18_446_744_073_709_551_616.0).contains(&value) {
                Ok(value as u64)
            } else {
                Err(FaultCode::Overflow)
            }
        }
        (F64, BYTE) => {
            let value = f64::from_bits(bits);
            if (0.0..256.0).contains(&value) {
                Ok(value as u8 as u64)
            } else {
                Err(FaultCode::Overflow)
            }
        }
        _ => Err(FaultCode::IllegalOperation),
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, FaultCode> {
    let end = offset.checked_add(4).ok_or(FaultCode::OutOfBounds)?;
    let slice = bytes.get(offset..end).ok_or(FaultCode::OutOfBounds)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, FaultCode> {
    let end = offset.checked_add(8).ok_or(FaultCode::OutOfBounds)?;
    let slice = bytes.get(offset..end).ok_or(FaultCode::OutOfBounds)?;
    let mut word = [0u8; 8];
    word.copy_from_slice(slice);
    Ok(u64::from_le_bytes(word))
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) -> Result<(), FaultCode> {
    let end = offset.checked_add(4).ok_or(FaultCode::OutOfBounds)?;
    let slice = bytes.get_mut(offset..end).ok_or(FaultCode::OutOfBounds)?;
    slice.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) -> Result<(), FaultCode> {
    let end = offset.checked_add(8).ok_or(FaultCode::OutOfBounds)?;
    let slice = bytes.get_mut(offset..end).ok_or(FaultCode::OutOfBounds)?;
    slice.copy_from_slice(&value.to_le_bytes());
    Ok(())
}
