//! Diagnostic: runs one admin operation through the same UAC path the app
//! uses (`elevate::run`), so failures in that path can be reproduced.
//!
//! cargo run -p serialloom-core --example elevate_probe -- rename-usb SERIAL=COM21

#[cfg(windows)]
fn main() {
    use serialloom_core::driver::native::{ADMIN_FLAG, admin_main, elevate};

    let args: Vec<String> = std::env::args().skip(1).collect();
    serialloom_core::logging::init("probe");
    if args.first().map(String::as_str) == Some(ADMIN_FLAG) {
        std::process::exit(admin_main(&args[1..]));
    }
    let op = match args.first().map(String::as_str) {
        Some("rename-usb") => {
            let pairs = args[1..]
                .iter()
                .filter_map(|a| a.split_once('=').map(|(s, n)| (s.to_string(), n.to_string())))
                .collect();
            elevate::AdminOp::RenameUsbPorts(pairs)
        }
        _ => {
            eprintln!("usage: elevate_probe rename-usb SERIAL=COMn");
            std::process::exit(2);
        }
    };
    // Like the app: run on a worker thread whose COM apartment is MTA.
    let mta = std::env::var("PROBE_MTA").is_ok();
    let handle = std::thread::spawn(move || {
        if mta {
            use windows_sys::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
            // SAFETY: initialises COM for this thread only.
            let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32) };
            println!("thread COM init (MTA): {hr:#x}");
        }
        elevate::run(op)
    });
    match handle.join().expect("thread") {
        Ok(_) => println!("OK"),
        Err(e) => println!("ERROR: {e}"),
    }
}

#[cfg(not(windows))]
fn main() {}
