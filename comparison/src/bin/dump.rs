use std::io::{self, Read};
use zsyn::Mode;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = match args.get(1).map(String::as_str) {
        Some("code") => Mode::Code,
        Some("math") => Mode::Math,
        _ => Mode::Markup,
    };
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    let ours = zsyn_comparison::parsed(&input, mode);
    let oracle = zsyn_comparison::oracle_parse(&input, mode);
    for (name, records) in [
        ("zsyn", zsyn_comparison::canonical(ours.as_tree())),
        ("typst-syntax", zsyn_comparison::oracle_canonical(&oracle)),
    ] {
        println!("{name}");
        for r in records {
            println!(
                "{}{} {}..{} {:?}",
                "  ".repeat(r.depth),
                r.kind,
                r.start,
                r.end,
                &input[r.start..r.end]
            );
        }
    }
}
