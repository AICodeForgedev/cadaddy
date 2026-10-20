//! The mutation fuzzer for the engine's own readers (`cargo xtask fuzz`):
//! `cargo run --release -p cadaddy-engine --example fuzz -- <xlsx|zip|lisp|all> [seconds] [out dir]`.
//! The file formats are fuzzed by `cadaddy-io`'s `fuzz` example; the engine shares its core.

#[path = "../../io/examples/common/fuzz_core.rs"]
mod fuzz_core;

const TARGETS: [&str; 3] = ["xlsx", "zip", "lisp"];

fn seeds(target: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    match target {
        "xlsx" => out.extend(cadaddy_engine::fuzz_xlsx_seed()),
        "zip" => {
            let entries = vec![("a.txt".to_string(), b"hello hello hello hello".to_vec()), ("Xrefs/b.dwg".to_string(), vec![7u8; 300])];
            out.extend(cadaddy_engine::cmd::file::zip::write(&entries).ok());
        }
        _ => out.push(
            br#"(defun c:test (/ a b) (setq a '(1 2.5 "s" (nested (deeper)))) (foreach x a (princ x)) (command "line" '(0 0) '(1 1) ""))
; a comment
(setq s "esc\"aped\\" q '(a . b) n -1.5e-3)"#
                .to_vec(),
        ),
    }
    out.extend(fuzz_core::corpus(match target {
        "xlsx" => &["xlsx"],
        "zip" => &["zip"],
        _ => &["lsp"],
    }));
    out
}

fn main() {
    fuzz_core::main_with(&fuzz_core::Harness { targets: &TARGETS, run: cadaddy_engine::fuzz_read, seeds, text: |t| t == "lisp" });
}
