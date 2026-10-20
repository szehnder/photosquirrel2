//! `--synthetic-catalog N` (ADR-0026): run the app on an N-item synthetic catalog built (once) by
//! `ps_service::library::synthetic`.

use ps_service::library::synthetic::ensure;

/// Build the catalog if needed, then run this binary again on it with the remaining arguments (so start-up
/// measurements time a clean launch). Returns the exit code.
pub fn rerun_on(items: u64) -> i32 {
    let dir = match ensure(items) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: building the synthetic catalog: {e:#}");
            return 1;
        }
    };
    let mut args: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a == "--synthetic-catalog" || a == "--data-dir" {
            it.next();
        } else if a.starts_with("--synthetic-catalog=") || a.starts_with("--data-dir=") {
        } else {
            args.push(a);
        }
    }
    let Ok(exe) = std::env::current_exe() else {
        return 1;
    };
    match std::process::Command::new(exe)
        .arg("--data-dir")
        .arg(&dir)
        .args(args)
        .status()
    {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}
