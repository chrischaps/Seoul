//! A stand-in Soba: sends the `--synth` track's link lanes to Seoul over OSC
//! without any audio, so you can try link presets over real music.
//!
//!     cargo run --release --example link_send            # port 9137
//!     cargo run --release --example link_send -- 9200
//!
//! Lanes, clock and gates are listed in `src/audio/link_fake.rs`.

#[path = "../src/audio/link_fake.rs"]
mod link_fake;

fn main() {
    let port = match std::env::args().nth(1) {
        None => 9137,
        Some(p) => p.parse().unwrap_or_else(|_| {
            eprintln!("usage: link_send [PORT]");
            std::process::exit(2);
        }),
    };
    println!("stand-in Soba → 127.0.0.1:{port} at 200 Hz, {} BPM (Ctrl+C to stop)", link_fake::BPM);
    if let Err(e) = link_fake::run(port, std::time::Instant::now(), "link_send example") {
        eprintln!("link_send: {e}");
        std::process::exit(1);
    }
}
