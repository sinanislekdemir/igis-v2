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
    begun: Vec<u64>,
    grab: Option<(i64, i64, i64, i64)>,
    last_grab: Option<(i64, i64, i64, i64)>,
    released: u32,
    plane: Option<(i64, i64, i64, i64)>,
    last_plane: Option<(i64, i64, i64, i64)>,
    plane_begun: Vec<u64>,
    plane_presented: Vec<u64>,
    plane_destroyed: Vec<u64>,
    work_area: Option<i64>,
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
            begun: Vec::new(),
            grab: None,
            last_grab: None,
            released: 0,
            plane: None,
            last_plane: None,
            plane_begun: Vec::new(),
            plane_presented: Vec::new(),
            plane_destroyed: Vec::new(),
            work_area: None,
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

    fn window_begin(&mut self, id: u64) -> Status {
        self.begun.push(id);
        if id == 0 || id == 7 {
            Status::Ok
        } else {
            Status::NotFound
        }
    }

    fn window_info(&mut self, index: u64) -> Option<WindowInfo> {
        if index == 0 {
            self.info
        } else {
            None
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

    fn plane_create(&mut self, x: i64, y: i64, w: i64, h: i64) -> Result<u64, Status> {
        self.plane = Some((x, y, w, h));
        self.last_plane = Some((x, y, w, h));
        Ok(42)
    }

    fn plane_destroy(&mut self, id: u64) -> Status {
        self.plane_destroyed.push(id);
        Status::Ok
    }

    fn plane_begin(&mut self, id: u64) -> Status {
        self.plane_begun.push(id);
        if id == 42 {
            Status::Ok
        } else {
            Status::NotFound
        }
    }

    fn plane_present(&mut self, id: u64) -> Status {
        self.plane_presented.push(id);
        if id == 42 {
            Status::Ok
        } else {
            Status::NotFound
        }
    }

    fn work_area_reserve(&mut self, height: i64) -> Status {
        self.work_area = Some(height);
        Status::Ok
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

    let mut instructions = Vec::new();
    op_const(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_BEGIN,
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
        "the default window_begin is the disabled service"
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
fn window_begin_pops_the_id_and_pushes_the_status() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 7);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_BEGIN,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_const(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_BEGIN,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op_const(&mut instructions, 9);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WINDOW_BEGIN,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 16);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(host.begun, vec![7, 0, 9]);
    assert_eq!(state_word(&state, 0), Status::Ok as u64);
    assert_eq!(state_word(&state, 8), Status::Ok as u64);
    assert_eq!(state_word(&state, 16), Status::NotFound as u64);
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
fn plane_builtins_pass_the_rect_and_write_the_id_on_ok() {
    let mut instructions = Vec::new();
    for value in [10, 20, 100, 50] {
        op_const(&mut instructions, value);
    }
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PLANE_CREATE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    // Begin, present and destroy the returned id.
    op_imm(&mut instructions, opcodes::LOAD_STATE, 16);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PLANE_BEGIN,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op_imm(&mut instructions, opcodes::LOAD_STATE, 16);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PLANE_PRESENT,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 24);
    op_imm(&mut instructions, opcodes::LOAD_STATE, 16);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PLANE_DESTROY,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 32);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = WindowHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(
        host.last_plane,
        Some((10, 20, 100, 50)),
        "the four coordinates arrive in declaration order"
    );
    assert_eq!(
        state_word(&state, 16),
        42,
        "the host id is written only on Ok"
    );
    assert_eq!(host.plane_begun, vec![42]);
    assert_eq!(host.plane_presented, vec![42]);
    assert_eq!(host.plane_destroyed, vec![42]);
    assert_eq!(state_word(&state, 0), Status::Ok as u64);
    assert_eq!(state_word(&state, 8), Status::Ok as u64);
    assert_eq!(state_word(&state, 24), Status::Ok as u64);
    assert_eq!(state_word(&state, 32), Status::Ok as u64);
}

#[test]
fn plane_create_rejects_a_non_positive_extent_before_the_host_call() {
    let mut instructions = Vec::new();
    for value in [0, 0, 1024, 0] {
        op_const(&mut instructions, value);
    }
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PLANE_CREATE,
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
    assert_eq!(host.plane, None, "the host is never called");
    assert_eq!(state_word(&state, 0), Status::Invalid as u64);
}

#[test]
fn the_default_plane_calls_and_work_area_are_the_disabled_service() {
    let mut instructions = Vec::new();
    for value in [0, 0, 10, 10] {
        op_const(&mut instructions, value);
    }
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PLANE_CREATE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_const(&mut instructions, 1);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PLANE_BEGIN,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 16);
    op_const(&mut instructions, 1);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PLANE_PRESENT,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 24);
    op_const(&mut instructions, 1);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PLANE_DESTROY,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 32);
    op_const(&mut instructions, 22);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WORK_AREA_RESERVE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 64);
    let mut state = vec![0u8; 64];
    let mut host = DisabledHost;
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), Status::Invalid as u64);
    assert_eq!(state_word(&state, 16), Status::Invalid as u64);
    assert_eq!(state_word(&state, 24), Status::Invalid as u64);
    assert_eq!(
        state_word(&state, 32),
        Status::NotFound as u64,
        "an unknown plane destroy is NotFound"
    );
    assert_eq!(state_word(&state, 40), Status::Invalid as u64);
}

#[test]
fn work_area_reserve_passes_the_height_and_rejects_a_negative_one() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 22);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WORK_AREA_RESERVE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_const(&mut instructions, -1);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::WORK_AREA_RESERVE,
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
    assert_eq!(host.work_area, Some(22), "the negative height is not passed");
    assert_eq!(state_word(&state, 0), Status::Ok as u64);
    assert_eq!(state_word(&state, 8), Status::Invalid as u64);
}

/// A mock console-session service (spec 0021).
struct SessionHost {
    sent: Vec<u8>,
    receive: Vec<u8>,
}

impl Host for SessionHost {
    fn console_write(&mut self, _bytes: &[u8]) {}

    fn clock(&mut self) -> u64 {
        0
    }

    fn session_open(&mut self) -> Result<u64, Status> {
        Ok(5)
    }

    fn session_close(&mut self, _id: u64) -> Status {
        Status::Ok
    }

    fn session_send(&mut self, _id: u64, data: &[u8]) -> Result<usize, Status> {
        self.sent.extend_from_slice(data);
        Ok(data.len())
    }

    fn session_recv(&mut self, _id: u64, dst: &mut [u8]) -> Result<usize, Status> {
        let count = self.receive.len().min(dst.len());
        dst[..count].copy_from_slice(&self.receive[..count]);
        self.receive.drain(..count);
        Ok(count)
    }

    fn session_readable(&mut self, _id: u64) -> bool {
        !self.receive.is_empty()
    }
}

#[test]
fn session_builtins_decode_and_publish_their_housekeeping() {
    let mut instructions = Vec::new();
    // id := session_open()
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::SESSION_OPEN,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    // sent := session_send(5, "abc", var sent)
    op_const(&mut instructions, 5);
    op_imm(&mut instructions, opcodes::REF_STATE, 32);
    op_imm(&mut instructions, opcodes::REF_STATE, 48);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::SESSION_SEND,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 56);
    // read := session_recv(5, var dst, var read)
    op_const(&mut instructions, 5);
    op_const(&mut instructions, 16);
    op_imm(&mut instructions, opcodes::REF_STATE, 80);
    op_imm(&mut instructions, opcodes::REF_STATE, 96);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::SESSION_RECV,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 104);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &[], 256);
    let mut state = vec![0u8; 256];
    // "abc" at offset 32; its length word is at 24.
    state[24..32].copy_from_slice(&3u64.to_le_bytes());
    state[32..35].copy_from_slice(b"abc");
    let mut host = SessionHost {
        sent: Vec::new(),
        receive: b"hi".to_vec(),
    };
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(interpreter::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), 5);
    assert_eq!(state_word(&state, 8), Status::Ok as u64);
    assert_eq!(host.sent, b"abc");
    assert_eq!(state_word(&state, 48), 3);
    assert_eq!(state_word(&state, 56), Status::Ok as u64);
    assert_eq!(state_word(&state, 96), 2);
    assert_eq!(state_word(&state, 104), Status::Ok as u64);
    // The receive destination publishes its runtime length and bytes.
    assert_eq!(state_word(&state, 72), 2);
    assert_eq!(&state[80..82], b"hi");
}

#[test]
fn the_default_session_service_is_disabled() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::SESSION_OPEN,
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
    assert_eq!(state_word(&state, 0), 0, "the id stays untouched");
    assert_eq!(state_word(&state, 8), Status::Invalid as u64);
}
