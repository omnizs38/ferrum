//! Ferrum UCI entry point.
//!
//! The binary is a thin wrapper: all logic lives in `ferrum::uci` so the
//! engine can be tested and embedded as a library. It prints the variant
//! name at startup (stderr only — the UCI handshake uses `uciok`).
//!
//! CLI flags are convenience for server deployments that would otherwise
//! have to send `setoption` before the first `go`:
//!
//! ```text
//! ferrum [--hash MB] [--threads N] [--eval-params SPEC] [--search-params SPEC]
//! ferrum --version | --help
//! ```
//!
//! Without flags it reads UCI from stdin and writes to stdout.

use ferrum::eval::EvalParams;
use ferrum::search::SearchParams;
use ferrum::uci;

const HELP: &str = "\
Ferrum — a from-scratch UCI chess engine

Usage:
  ferrum [options]            run the UCI loop on stdin/stdout
  ferrum --version            print the version and exit
  ferrum --help               print this help and exit

Options:
  --hash MB                   default transposition table size (1..max)
  --threads N                 default thread count (1..max; Alpha is always 1)
  --eval-params SPEC          initial EvalParams (key=value,... or 'default')
  --search-params SPEC        initial SearchParams (key=value,... or 'default')
";

fn take_value(args: &[String], i: &mut usize, arg: &str, what: &str) -> String {
    *i += 1;
    match args.get(*i) {
        Some(v) => v.clone(),
        None => {
            eprintln!("ferrum: {arg} needs a {what}");
            std::process::exit(2);
        }
    }
}

fn main() {
    let mut config = uci::Config::default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--help" | "-h" => {
                println!("{HELP}");
                return;
            }
            "--version" | "-V" => {
                let variant = if cfg!(feature = "gamma") { "gamma" } else { "alpha" };
                println!("ferrum {} ({variant})", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--hash" => match take_value(&args, &mut i, arg, "size in MB").parse::<usize>() {
                Ok(v) => config.hash_mb = v,
                Err(_) => {
                    eprintln!("ferrum: --hash expects an integer");
                    std::process::exit(2);
                }
            },
            "--threads" => match take_value(&args, &mut i, arg, "thread count").parse::<usize>() {
                Ok(v) => config.threads = v,
                Err(_) => {
                    eprintln!("ferrum: --threads expects an integer");
                    std::process::exit(2);
                }
            },
            "--eval-params" => {
                let spec = take_value(&args, &mut i, arg, "spec");
                match EvalParams::from_spec(&spec) {
                    Ok(p) => config.eval = p,
                    Err(e) => {
                        eprintln!("ferrum: --eval-params: {e}");
                        std::process::exit(2);
                    }
                }
            }
            "--search-params" => {
                let spec = take_value(&args, &mut i, arg, "spec");
                match SearchParams::from_spec(&spec) {
                    Ok(p) => config.search = p,
                    Err(e) => {
                        eprintln!("ferrum: --search-params: {e}");
                        std::process::exit(2);
                    }
                }
            }
            other => {
                eprintln!("ferrum: unknown argument '{other}' (try --help)");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    // Greet on stderr so stdout stays clean UCI. GUIs ignore stderr.
    #[cfg(feature = "gamma")]
    eprintln!("Ferrum Gamma (standard)");
    #[cfg(not(feature = "gamma"))]
    eprintln!("Ferrum Alpha (lightweight)");
    uci::run_with_config(std::io::stdin().lock(), std::io::stdout(), config);
}
