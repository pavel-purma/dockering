//! SHL-022 / UPD-017: the relaunch wait runs before anything reads state.
//!
//! `main()` has to call, in this order (spec 10 §7): the panic hook, the relaunch wait, the config
//! load, logging, the single-instance check. The wait must come before `load_config` or the old
//! instance's final save of `config.toml` / `state.json` could be overwritten by a stale copy, and
//! before `single_instance::acquire` or the new process would ask the dying one to focus and exit.
//! Nothing at run time can notice a reordering (no test can run `main()`), so this scan reads
//! `crates/dockering/src/main.rs` and checks the order of the first call of each.
//!
//! It lives in xtask, not next to the code: `scripts/check-blocking.sh` scans whole files under
//! `crates/dockering/src`, tests included, and a scan needs file-system access.

use std::fs;

use crate::util::workspace_root;

/// The app binary's `main()`.
const MAIN_RS: &str = "crates/dockering/src/main.rs";

/// What `main()` calls, in the order spec 10 §7 requires. A name matches a call when it is not the
/// tail of a longer identifier and is followed (after the rest of its identifier) by `(`, so
/// `wait_for_predecessor` also finds `wait_for_predecessor_from_args(…)`.
const ORDER: &[&str] = &[
    "install_panic_hook",
    "wait_for_predecessor",
    "load_config",
    "logging::init",
    "single_instance::acquire",
];

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `source` with comments and the insides of string and character literals blanked out, so a name
/// in a comment or a message is never mistaken for a call. Same byte length and the same line
/// breaks, so offsets in the result are offsets in `source`.
fn code_only(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let blank = |out: &mut [u8], from: usize, to: usize| {
        for byte in &mut out[from..to.min(bytes.len())] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                let end = bytes[i..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |p| i + p);
                blank(&mut out, i, end);
                i = end;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let (mut depth, mut j) = (1, i + 2);
                while j < bytes.len() && depth > 0 {
                    if bytes[j] == b'/' && bytes.get(j + 1) == Some(&b'*') {
                        depth += 1;
                        j += 2;
                    } else if bytes[j] == b'*' && bytes.get(j + 1) == Some(&b'/') {
                        depth -= 1;
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                blank(&mut out, i, j);
                i = j;
            }
            b'"' => {
                // `r"…"` / `r#"…"#`: the `#`s and an `r` directly before the quote.
                let mut start = i;
                let mut hashes = 0;
                while start > 0 && bytes[start - 1] == b'#' {
                    start -= 1;
                    hashes += 1;
                }
                let raw = start > 0 && bytes[start - 1] == b'r';
                let mut j = i + 1;
                while j < bytes.len() {
                    if raw {
                        if bytes[j] == b'"'
                            && bytes[j + 1..].iter().take_while(|&&b| b == b'#').count() >= hashes
                        {
                            break;
                        }
                    } else if bytes[j] == b'\\' {
                        j += 1;
                    } else if bytes[j] == b'"' {
                        break;
                    }
                    j += 1;
                }
                blank(&mut out, i + 1, j);
                i = j + 1;
            }
            b'\'' => {
                // A character literal (`'x'`, `'\n'`, `'"'`) or a lifetime (`'a`).
                let escaped = bytes.get(i + 1) == Some(&b'\\');
                let width = source[i + 1..].chars().next().map_or(0, char::len_utf8);
                if escaped {
                    let close = bytes[i + 2..].iter().position(|&b| b == b'\'');
                    let end = close.map_or(bytes.len(), |p| i + 2 + p);
                    blank(&mut out, i + 1, end);
                    i = end + 1;
                } else if bytes.get(i + 1 + width) == Some(&b'\'') {
                    blank(&mut out, i + 1, i + 1 + width);
                    i += width + 2;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out).expect("blanking whole literals keeps the text valid UTF-8")
}

/// Byte offset of the first call of `name` in `code` (comments and literals already blanked).
fn first_call(code: &str, name: &str) -> Option<usize> {
    let bytes = code.as_bytes();
    code.match_indices(name).map(|(at, _)| at).find(|&at| {
        if at > 0 && is_ident(bytes[at - 1]) {
            return false;
        }
        let mut rest = at + name.len();
        while rest < bytes.len() && is_ident(bytes[rest]) {
            rest += 1;
        }
        while rest < bytes.len() && bytes[rest].is_ascii_whitespace() {
            rest += 1;
        }
        bytes.get(rest) == Some(&b'(')
    })
}

/// Ok when every call of [`ORDER`] is in `source` and in that order.
fn check_order(source: &str) -> Result<(), String> {
    let code = code_only(source);
    let mut previous: Option<(&str, usize)> = None;
    for &name in ORDER {
        let Some(at) = first_call(&code, name) else {
            return Err(format!("`{name}(…)` is never called ({MAIN_RS})"));
        };
        if let Some((before, before_at)) = previous
            && at < before_at
        {
            return Err(format!(
                "`{name}` (byte {at}) is called before `{before}` (byte {before_at}); \
                 spec 10 §7 requires: {}",
                ORDER.join(" < ")
            ));
        }
        previous = Some((name, at));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `main()` in the documented order, with the names also mentioned where they must not count.
    const GOOD: &str = r#"
        // load_config(&paths) comes later; single_instance::acquire(&paths) after that
        fn main() {
            let msg = "call wait_for_predecessor(…) first";
            dk_hub::logging::install_panic_hook(&paths);
            /* load_config( is read after the wait */
            let wait = dk_hub::single_instance::wait_for_predecessor_from_args(&argv);
            let (config, ui_state) = dk_hub::load_config(&paths);
            let _logging = dk_hub::logging::init(&paths, level);
            match dk_hub::single_instance::acquire(&paths) { _ => {} }
        }
    "#;

    #[test]
    fn shl_022_wait_runs_before_config_load() {
        let path = workspace_root().join(MAIN_RS);
        let source =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        if let Err(problem) = check_order(&source) {
            panic!(
                "{problem}\nThe relaunch wait must come right after the panic hook and before \
                 the config is read (SHL-022, UPD-017)."
            );
        }
    }

    #[test]
    fn shl_022_order_scan_accepts_the_documented_order() {
        assert_eq!(check_order(GOOD), Ok(()));
    }

    #[test]
    fn shl_022_order_scan_detects_a_wrong_order() {
        // Config loaded before the wait.
        let swapped = "install_panic_hook(a); load_config(b); wait_for_predecessor_from_args(c); \
                       logging::init(d); single_instance::acquire(e);";
        let err = check_order(swapped).unwrap_err();
        assert!(
            err.contains("`load_config`") && err.contains("`wait_for_predecessor`"),
            "{err}"
        );

        // The wait after the single-instance check.
        let late = r#"
            fn main() {
                install_panic_hook(&p);
                load_config(&p);
                logging::init(&p, l);
                single_instance::acquire(&p);
                wait_for_predecessor_from_args(&a);
            }
        "#;
        assert!(check_order(late).is_err());

        // Logging before the config (a different pair of the order).
        let logging_first = "install_panic_hook(a); wait_for_predecessor(b); logging::init(c); \
                             load_config(d); single_instance::acquire(e);";
        let err = check_order(logging_first).unwrap_err();
        assert!(
            err.contains("`load_config`") && err.contains("`logging::init`"),
            "{err}"
        );
    }

    #[test]
    fn shl_022_order_scan_detects_a_missing_call() {
        let no_wait = "install_panic_hook(a); load_config(b); logging::init(c); \
                       single_instance::acquire(d);";
        let err = check_order(no_wait).unwrap_err();
        assert!(err.contains("wait_for_predecessor"), "{err}");

        // A mention in a comment or a string is not a call.
        let only_mentioned = "install_panic_hook(a);\n// wait_for_predecessor(x)\n\
                              let s = \"wait_for_predecessor(y)\";\nload_config(b); \
                              logging::init(c); single_instance::acquire(d);";
        assert!(check_order(only_mentioned).is_err());

        // A longer name that merely ends in a needle is a different function.
        let lookalike = "install_panic_hook(a); my_wait_for_predecessor(x); load_config(b); \
                         logging::init(c); single_instance::acquire(d);";
        assert!(check_order(lookalike).is_err());
    }

    #[test]
    fn shl_022_order_scan_blanks_comments_and_literals_without_moving_offsets() {
        let source = "a // c1 \"x\n\
                      b /* c2 /* nested */ still */ c\n\
                      let s = \"q\\\"load_config(\"; let r = r#\"wait_for_predecessor(\"#;\n\
                      let c = '\"'; let l: &'static str = \"\"; // end\n\
                      d(); é(); \"é\"";
        let code = code_only(source);
        assert_eq!(code.len(), source.len());
        assert_eq!(code.matches('\n').count(), source.matches('\n').count());
        assert!(!code.contains("c1") && !code.contains("c2") && !code.contains("nested"));
        assert!(!code.contains("load_config") && !code.contains("wait_for_predecessor"));
        // Code outside the comments and literals is untouched, offsets included.
        for kept in ["a ", "\nb ", " c\n", "let s = ", "'static", "d();", "é();"] {
            let at = source.find(kept).unwrap();
            assert_eq!(&code[at..at + kept.len()], kept, "{kept:?}");
        }
    }
}
