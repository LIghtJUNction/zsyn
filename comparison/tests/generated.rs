#![cfg(feature = "oracle")]
use zsyn::Mode;
use zsyn_comparison::equivalence;

fn step(state: &mut u64) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state as usize
}

fn expression(state: &mut u64, depth: usize) -> String {
    if depth == 0 {
        let leaves = ["x", "123", "true", "\"café\"", "none", "auto", "12pt"];
        return leaves[step(state) % leaves.len()].to_owned();
    }
    match step(state) % 9 {
        0 => format!("({})", expression(state, depth - 1)),
        1 => format!(
            "({}, {})",
            expression(state, depth - 1),
            expression(state, depth - 1)
        ),
        2 => format!("(name: {})", expression(state, depth - 1)),
        3 => format!("{{ let x = {}; x }}", expression(state, depth - 1)),
        4 => format!("(x => {})", expression(state, depth - 1)),
        5 => format!(
            "f({}, size: {})",
            expression(state, depth - 1),
            expression(state, depth - 1)
        ),
        6 => format!("[*hello* #({})]", expression(state, depth - 1)),
        7 => format!("-({})", expression(state, depth - 1)),
        _ => {
            let operations = [
                "+", "-", "*", "/", "==", "!=", "<", "<=", ">", ">=", "and", "or", "in", "not in",
            ];
            let operation = operations[step(state) % operations.len()];
            format!(
                "({} {operation} {})",
                expression(state, depth - 1),
                expression(state, depth - 1)
            )
        }
    }
}

#[test]
fn deterministic_grammar_combinations_match_complete_trees() {
    let mut state = 0x670f_d5ba_8e92_43c1;
    for _ in 0..128 {
        let expr = expression(&mut state, 3);
        for (mode, source) in [
            (Mode::Code, format!("let value = {expr}; value")),
            (Mode::Markup, format!("Before #({expr}) after.")),
        ] {
            if let Err(error) = equivalence(&source, mode) {
                panic!("generated {mode:?} {source:?}: {error}");
            }
        }
    }
}
