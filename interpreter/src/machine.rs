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

use abi::opcodes::{self, builtin, value_kind};
use abi::{refs, ExitReason, FaultCode};

use crate::program::Program;

/// The host interface as far as this slice needs it (spec 0005).
///
/// The kernel will implement this over its own console and clock; the host
/// toolchain implements it over stdout and `Instant`. Neither the interpreter
/// nor the compiler knows which one is behind the trait.
pub trait Host {
    /// Writes application bytes to the console. The bytes are unvalidated:
    /// they may be arbitrary binary data.
    fn console_write(&mut self, bytes: &[u8]);

    /// Monotonic nanoseconds since boot (or since the host process started).
    fn clock(&mut self) -> u64;
}

/// Result of one [`Machine::run`] slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The step budget ran out (or a yield was requested); call `run` again to
    /// resume at the same instruction boundary.
    Yielded,
    /// The task stopped. No further execution is possible.
    Exited(ExitReason),
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
        }
    }

    /// Returns the exit reason if the machine has stopped.
    pub fn halted(&self) -> Option<ExitReason> {
        self.halted
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
            match self.step(program, host, state, stack, args) {
                Ok(Some(reason)) => {
                    self.halted = Some(reason);
                    return Outcome::Exited(reason);
                }
                Ok(None) => {}
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
    ) -> Result<Option<ExitReason>, FaultCode> {
        let code = program.instructions();
        let constants = program.constants();
        let pc = self.pc;
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
                    return Ok(Some(reason));
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
        Ok(None)
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
