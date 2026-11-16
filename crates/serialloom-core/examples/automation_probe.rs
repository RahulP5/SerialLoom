//! Serves the automation API with the in-memory demo driver, so
//! `serialloom-cli` and other clients can be tried without the app and
//! without touching real ports.
//!
//!     cargo run -p serialloom-core --example automation_probe [seconds]
//!
//! Then, in another terminal: `cargo run -p serialloom-cli -- pair COM9 COM10`.
//! The real app must not be running with automation on (same pipe name).

#[cfg(windows)]
fn main() {
    use std::sync::Arc;
    use std::time::Duration;

    use serialloom_core::Engine;
    use serialloom_core::automation::AutomationServer;
    use serialloom_core::driver::demo::DemoDriver;

    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(60);
    let engine = Arc::new(Engine::new(Arc::new(DemoDriver::new()), None));
    let server = AutomationServer::start(Arc::clone(&engine), Arc::new(|| println!("(topology changed)")))
        .expect("start the automation API");
    println!("serving {} for {secs} s (demo driver)", server.pipe_name());
    std::thread::sleep(Duration::from_secs(secs));
    println!("hubs at exit: {}", engine.hubs().len());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}
