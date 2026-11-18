//! Window builtin tests with a mock service (spec 0011).
//!
//! The interpreter does not own windows: it decodes the operands, resolves the
//! title and the out-parameters, calls the host and writes results only on
//! `Ok`. These tests pin that contract without the kernel, and they exercise
//! the disabled-service defaults every other host inherits.

use abi::layout;
use abi::opcodes;
use abi::{
    CodeHeader, Function, Rect, Status, ABI_VERSION, CODE_MAGIC, NO_DIAGNOSTIC, NO_NAME,
    PROFILE_X86_64_LE,
};
use interpreter::{Host, Machine, Outcome, WindowInfo};

fn build(instructions: &[u8], constants: &[u8], state_size: u32) -> Vec<u8> {
    let header = CodeHeader {
        magic: CODE_MAGIC,
        abi_version: ABI_VERSION,
        profile: PROFILE_X86_64_LE,
        function_count: 1,
        entry: 0,
        code_len: instructions.len() as u32,
        const_len: constants.len() as u32,
        line_len: 0,
        state_size,
        stack_size: 256,
        diag_offset: NO_DIAGNOSTIC,
    };
    let mut code = Vec::new();
    code.extend_from_slice(&header.to_bytes());
    code.extend_from_slice(
        &Function {
            code_off: 0,
            frame_size: 192,
            operand_off: 40,
            line_base: 0,
            name_off: NO_NAME,
            param_count: 1,
            has_destination: false,
            _reserved: [0, 0],
        }
        .to_bytes(),
    );
    while code.len() % 8 != 0 {
        code.push(0);
    }
    code.extend_from_slice(instructions);
    while code.len() % 8 != 0 {
        code.push(0);
    }
    code.extend_from_slice(constants);
    while code.len() % 8 != 0 {
        code.push(0);
    }
    code
}

fn op(out: &mut Vec<u8>, opcode: u8) {
    out.extend_from_slice(&[opcode, 0, 0, 0]);
}

fn op_imm(out: &mut Vec<u8>, opcode: u8, immediate: u32) {
    op(out, opcode);
    out.extend_from_slice(&immediate.to_le_bytes());
}

fn op_const(out: &mut Vec<u8>, value: i64) {
    op(out, opcodes::CONST);
    out.extend_from_slice(&(value as u32).to_le_bytes());
    out.extend_from_slice(&((value >> 32) as u32).to_le_bytes());
}

fn constant(data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u64).to_le_bytes().to_vec();
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(8) {
        out.push(0);
    }
    out
}

/// A mock window service that records calls and returns scripted results.
struct WindowHost {
    open: Result<u64, Status>,
    rect: Result<Rect, Status>,
    focus: u64,
    info: Option<WindowInfo>,
    opened: Option<(i64, i64, i64, i64, Vec<u8>)>,
    moved: Option<(u64, i64, i64)>,
    closed: Vec<u64>,
    raised: Vec<u64>,
    presented: Vec<u64>,
    attached: Vec<u64>,
    grab: Option<(i64, i64, i64, i64)>,
    last_grab: Option<(i64, i64, i64, i64)>,
    released: u32,
    region: Option<(i64, i64, i64, i64)>,
    last_region: Option<(i64, i64, i64, i64)>,
    region_released: u32,
}

impl WindowHost {
    fn new() -> Self {
        WindowHost {
            open: Ok(7),
            rect: Ok(Rect {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
            }),
            focus: 7,
            info: None,
            opened: None,
            moved: None,
            closed: Vec::new(),
            raised: Vec::new(),
            presented: Vec::new(),
            attached: Vec::new(),
            grab: None,
            last_grab: None,
            released: 0,
            region: None,
            last_region: None,
            region_released: 0,
        }
    }
}

impl Host for WindowHost {
    fn console_write(&mut self, _bytes: &[u8]) {}

    fn clock(&mut self) -> u64 {
        0
    }

    fn window_open(&mut self, x: i64, y: i64, w: i64, h: i64, title: &[u8]) -> Result<u64, Status> {
        self.opened = Some((x, y, w, h, title.to_vec()));
        self.open
    }

    fn window_close(&mut self, id: u64) -> Status {
        self.closed.push(id);
        Status::Ok
    }

    fn window_raise(&mut self, id: u64) -> Status {
        self.raised.push(id);
        Status::Ok
    }

    fn window_move(&mut self, id: u64, x: i64, y: i64) -> Status {
        self.moved = Some((id, x, y));
        Status::Ok
    }

    fn window_frame_rect(&mut self, _id: u64) -> Result<Rect, Status> {
        self.rect
    }

    fn window_content_rect(&mut self, _id: u64) -> Result<Rect, Status> {
        self.rect
    }

    fn window_focus(&mut self) -> u64 {
        self.focus
    }

    fn window_present(&mut self, id: u64) -> Status {
        self.presented.push(id);
        if id == 0 {
            Status::NotFound
        } else {
            Status::Ok
        }
    }

    fn window_info(&mut self, index: u64) -> Option<WindowInfo> {
        if index == 0 {
            self.info
        } else {
            None
        }
    }

    fn terminal_attach(&mut self, window: u64) -> Status {
        self.attached.push(window);
        if window == 0 {
            Status::NotFound
        } else {
            Status::Ok
        }
    }

    fn popup_grab(&mut self, x: i64, y: i64, w: i64, h: i64) -> Status {
        self.grab = Some((x, y, w, h));
        self.last_grab = Some((x, y, w, h));
        Status::Ok
    }

    fn popup_release(&mut self) -> Status {
        self.released += 1;
        if self.grab.take().is_some() {
            Status::Ok
        } else {
            Status::NotFound
        }
    }

    fn region_claim(&mut self, x: i64, y: i64, w: i64, h: i64) -> Status {
        self.region = Some((x, y, w, h));
        self.last_region = Some((x, y, w, h));
        Status::Ok
    }

    fn region_release(&mut self) -> Status {
        self.region_released += 1;
        if self.region.take().is_some() {
            Status::Ok
        } else {
            Status::NotFound
        }
    }
}

/// A host with no window overrides at all: the trait defaults.
struct DisabledHost;

impl Host for DisabledHost {
    fn console_write(&mut self, _bytes: &[u8]) {}

    fn clock(&mut self) -> u64 {
        0
    }
}

fn run(code: &[u8], state: &mut [u8], host: &mut impl Host) -> Outcome {
    let mut scratch = vec![0u8; code.len().div_ceil(32)];
    let program = interpreter::Program::parse(code, &mut scratch).expect("package verifies");
    let mut stack = vec![0u8; 256];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    machine.run(&program, host, state, &mut stack, &args, 10_000)
}

fn state_word(state: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(state[offset..offset + 8].try_into().unwrap())
}

#[test]
fn window_open_passes_geometry_and_title_and_writes_the_id_on_ok() {
    let mut instructions = Vec::new();
    for value in [10, 20, 100, 50] {
        op_const(&mut instructions, value);
    }
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_OPEN,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &constant(b"one"), 64);
    let mut state = vec![0u8; 64];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), 7, "the id is written on Ok");
    assert_eq!(state_word(&state, 40), Status::Ok as u64);
    assert_eq!(
        host.opened,
        Some((10, 20, 100, 50, b"one".to_vec())),
        "operands arrive in declaration order"
    );
}

#[test]
fn window_open_errors_leave_the_id_untouched() {
    let mut instructions = Vec::new();
    for value in [0, 0, 4, 4] {
        op_const(&mut instructions, value);
    }
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_OPEN,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &constant(b"x"), 64);
    let mut state = vec![0u8; 64];
    state[0..8].copy_from_slice(&0xAAu64.to_le_bytes());
    let mut host = WindowHost::new();
    host.open = Err(Status::Full);
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), 0xAA, "the id is untouched on error");
    assert_eq!(state_word(&state, 40), Status::Full as u64);
}

#[test]
fn window_frame_rect_writes_the_predeclared_rect_image_on_ok() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 7);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_FRAME_RECT,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 40), Status::Ok as u64);
    assert_eq!(
        layout::rect::read(&state[8..40]),
        Some(Rect {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
        })
    );
}

#[test]
fn window_content_rect_errors_leave_the_destination_untouched() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 99);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_CONTENT_RECT,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0xAAu8; 64];
    let mut host = WindowHost::new();
    host.rect = Err(Status::NotFound);
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 40), Status::NotFound as u64);
    assert_eq!(&state[8..40], &[0xAA; 32]);
}

#[test]
fn window_move_close_raise_and_focus_decode_their_operands() {
    let mut instructions = Vec::new();
    // move(5, 30, 40)
    op_const(&mut instructions, 5);
    op_const(&mut instructions, 30);
    op_const(&mut instructions, 40);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_MOVE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    // close(5)
    op_const(&mut instructions, 5);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_CLOSE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 48);
    // raise(5)
    op_const(&mut instructions, 5);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_RAISE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 56);
    // focus()
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_FOCUS,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(host.moved, Some((5, 30, 40)));
    assert_eq!(host.closed, vec![5]);
    assert_eq!(host.raised, vec![5]);
    assert_eq!(state_word(&state, 40), Status::Ok as u64);
    assert_eq!(state_word(&state, 48), Status::Ok as u64);
    assert_eq!(state_word(&state, 56), Status::Ok as u64);
    assert_eq!(state_word(&state, 0), 7, "focus is an int");
}

#[test]
fn the_trait_defaults_are_a_disabled_window_service() {
    let mut instructions = Vec::new();
    for value in [10, 20, 100, 50] {
        op_const(&mut instructions, value);
    }
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_OPEN,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    op_const(&mut instructions, 1);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_FRAME_RECT,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 48);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_FOCUS,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 56);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &constant(b"x"), 64);
    let mut state = vec![0u8; 64];
    let mut host = DisabledHost;
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 40), Status::Invalid as u64);
    assert_eq!(state_word(&state, 48), Status::Invalid as u64);
    assert_eq!(state_word(&state, 56), 0);
    let mut instructions = Vec::new();
    op_const(&mut instructions, 1);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_PRESENT,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = DisabledHost;
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(
        state_word(&state, 0),
        Status::Invalid as u64,
        "the default window_present is the disabled service"
    );
}

#[test]
fn window_present_pops_the_id_and_pushes_the_status() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 5);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_PRESENT,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_const(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_PRESENT,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(host.presented, vec![5, 0]);
    assert_eq!(state_word(&state, 0), Status::Ok as u64);
    assert_eq!(state_word(&state, 8), Status::NotFound as u64);
}

#[test]
fn window_info_writes_the_id_rect_and_title_on_ok() {
    // Operands: index, id reference, Rect reference, title data reference.
    // The title data starts at state 48, so its length word is state[40..48].
    let mut instructions = Vec::new();
    op_const(&mut instructions, 0);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 48);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_INFO,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 112);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 120);
    let mut state = vec![0u8; 120];
    let mut host = WindowHost::new();
    let mut title = [0u8; interpreter::WINDOW_TITLE_CAP];
    title[..4].copy_from_slice(b"main");
    host.info = Some(WindowInfo {
        id: 12,
        frame: Rect {
            x: 8,
            y: 26,
            w: 498,
            h: 695,
        },
        title_len: 4,
        title,
    });
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 112), Status::Ok as u64);
    assert_eq!(state_word(&state, 0), 12, "the id is written on Ok");
    assert_eq!(
        layout::rect::read(&state[8..40]),
        Some(Rect {
            x: 8,
            y: 26,
            w: 498,
            h: 695,
        })
    );
    assert_eq!(state_word(&state, 40), 4, "the title length is written");
    assert_eq!(&state[48..52], b"main");
    assert_eq!(&state[52..112], &[0u8; 60]);
}

#[test]
fn window_info_not_found_leaves_every_destination_untouched() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 7);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 48);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_INFO,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 112);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 120);
    let mut state = vec![0xAAu8; 120];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 112), Status::NotFound as u64);
    assert_eq!(&state[0..112], &[0xAA; 112]);
}

#[test]
fn the_default_window_info_is_the_disabled_service() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 0);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 48);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_INFO,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 112);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 120);
    let mut state = vec![0u8; 120];
    let mut host = DisabledHost;
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 112), Status::NotFound as u64);
    assert_eq!(
        state_word(&state, 0),
        0,
        "the disabled default writes nothing"
    );
}

#[test]
fn terminal_attach_pops_the_window_and_pushes_the_status() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 12);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::TERMINAL_ATTACH,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_const(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::TERMINAL_ATTACH,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(host.attached, vec![12, 0]);
    assert_eq!(state_word(&state, 0), Status::Ok as u64);
    assert_eq!(state_word(&state, 8), Status::NotFound as u64);
}

#[test]
fn the_default_terminal_attach_is_the_disabled_service() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 3);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::TERMINAL_ATTACH,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = DisabledHost;
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), Status::Invalid as u64);
}

#[test]
fn popup_grab_passes_the_rect_and_release_returns_the_host_status() {
    let mut instructions = Vec::new();
    for value in [10, 22, 176, 98] {
        op_const(&mut instructions, value);
    }
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POPUP_GRAB,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POPUP_RELEASE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(
        host.last_grab,
        Some((10, 22, 176, 98)),
        "the four coordinates arrive in declaration order"
    );
    assert_eq!(host.released, 1);
    assert_eq!(state_word(&state, 0), Status::Ok as u64);
    assert_eq!(state_word(&state, 8), Status::Ok as u64);
}

#[test]
fn popup_grab_rejects_a_non_positive_extent_before_the_host_call() {
    let mut instructions = Vec::new();
    for value in [10, 22, 0, 98] {
        op_const(&mut instructions, value);
    }
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POPUP_GRAB,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(host.grab, None, "the host is never called");
    assert_eq!(state_word(&state, 0), Status::Invalid as u64);
}

#[test]
fn the_default_popup_grab_and_release_are_the_disabled_service() {
    let mut instructions = Vec::new();
    for value in [0, 0, 10, 10] {
        op_const(&mut instructions, value);
    }
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POPUP_GRAB,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POPUP_RELEASE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = DisabledHost;
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), Status::Invalid as u64);
    assert_eq!(state_word(&state, 8), Status::Invalid as u64);
}

#[test]
fn region_claim_passes_the_rect_and_release_returns_the_host_status() {
    let mut instructions = Vec::new();
    for value in [0, 0, 1024, 22] {
        op_const(&mut instructions, value);
    }
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::REGION_CLAIM,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::REGION_RELEASE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(
        host.last_region,
        Some((0, 0, 1024, 22)),
        "the four coordinates arrive in declaration order"
    );
    assert_eq!(host.region_released, 1);
    assert_eq!(state_word(&state, 0), Status::Ok as u64);
    assert_eq!(state_word(&state, 8), Status::Ok as u64);
}

#[test]
fn region_claim_rejects_a_non_positive_extent_before_the_host_call() {
    let mut instructions = Vec::new();
    for value in [0, 0, 1024, 0] {
        op_const(&mut instructions, value);
    }
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::REGION_CLAIM,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(host.region, None, "the host is never called");
    assert_eq!(state_word(&state, 0), Status::Invalid as u64);
}

#[test]
fn the_default_region_claim_and_release_are_the_disabled_service() {
    let mut instructions = Vec::new();
    for value in [0, 0, 10, 10] {
        op_const(&mut instructions, value);
    }
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::REGION_CLAIM,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::REGION_RELEASE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = DisabledHost;
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), Status::Invalid as u64);
    assert_eq!(state_word(&state, 8), Status::Invalid as u64);
}
