// SPDX-License-Identifier: MIT OR Apache-2.0

#[path = "../kernel/src/bsp/_x86/script.rs"]
mod script;
use std::cell::RefCell;

thread_local! { static OUTPUT: RefCell<Vec<u8>> = RefCell::new(Vec::new()); }
fn emit(pointer: *const u8, length: usize) {
    OUTPUT.with(|output| output.borrow_mut().extend_from_slice(unsafe { std::slice::from_raw_parts(pointer, length) }));
}
fn run(source: &str) -> (Result<(), script::Error>, String) {
    OUTPUT.with(|output| output.borrow_mut().clear());
    let result = unsafe { script::run(source.as_ptr(), source.len(), emit) };
    let output = OUTPUT.with(|output| String::from_utf8(output.borrow().clone()).unwrap());
    (result, output)
}

#[test]
fn variables_arithmetic_and_precedence() {
    let (result, output) = run("let x = 2 + 3 * 4; print(x); x = (x - 2) / 3; print(x); print(-42);");
    assert_eq!(result, Ok(()));
    assert_eq!(output, "14\r\n4\r\n-42\r\n");
}

#[test]
fn branches_loops_strings_and_comments() {
    let (result, output) = run("// demo\nlet n = 0; while (n < 3) { if (n == 1) { print(\"middle\"); } else { print(n); } n = n + 1; }");
    assert_eq!(result, Ok(()));
    assert_eq!(output, "0\r\nmiddle\r\n2\r\n");
}

#[test]
fn inactive_branches_do_not_evaluate_names_or_divide() {
    assert_eq!(run("if (0) { print(missing / 0); } else { print(1); }").0, Ok(()));
    assert_eq!(run("while (0) { print(missing); }").0, Ok(()));
}

#[test]
fn errors_have_lines_and_distinct_kinds() {
    assert_eq!(run("print(1);\nprint(1 / 0);").0, Err(script::Error { line: 2, code: script::ARITHMETIC }));
    assert_eq!(run("\nprint(missing);").0, Err(script::Error { line: 2, code: script::NAME }));
    assert_eq!(run("\nprint(1)").0, Err(script::Error { line: 2, code: script::SYNTAX }));
    assert_eq!(run("\nprint(\"unterminated)").0, Err(script::Error { line: 2, code: script::LEXICAL }));
}

#[test]
fn infinite_loop_and_nesting_are_bounded() {
    assert_eq!(run("while (1) { }").0.unwrap_err().code, script::LIMIT);
    assert_eq!(run(&format!("print({}1{});", "(".repeat(32), ")".repeat(32))).0.unwrap_err().code, script::LIMIT);
}

#[test]
fn overflow_duplicate_and_unknown_names_are_rejected() {
    assert_eq!(run("print(9223372036854775807 + 1);").0.unwrap_err().code, script::ARITHMETIC);
    assert_eq!(run("let x = 1; let x = 2;").0.unwrap_err().code, script::NAME);
    assert_eq!(run("x = 1;").0.unwrap_err().code, script::NAME);
}

#[test]
fn variable_capacity_is_exact() {
    let source = (0..32).map(|index| format!("let v{} = {};", index, index)).collect::<String>();
    let (result, output) = run(&(source.clone() + "print(v31);"));
    assert_eq!(result, Ok(()));
    assert_eq!(output, "31\r\n");
    assert_eq!(run(&(source + "let extra = 1;")).0.unwrap_err().code, script::LIMIT);
}
