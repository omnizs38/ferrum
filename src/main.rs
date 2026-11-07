//! Ferrum UCI entry point.
//!
//! The binary is a thin wrapper: all logic lives in `ferrum::uci` so the
//! engine can be tested as a library. It prints the variant name at startup
//! (informational only — the UCI handshake uses `uciok`).

use ferrum::uci;

fn main() {
    // Greet on stderr so stdout stays clean UCI. GUIs ignore stderr.
    #[cfg(feature = "gamma")]
    eprintln!("Ferrum Gamma (standard)");
    #[cfg(not(feature = "gamma"))]
    eprintln!("Ferrum Alpha (lightweight)");
    uci::run();
}
