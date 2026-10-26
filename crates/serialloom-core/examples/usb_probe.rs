//! Diagnostic: runs the app's virtual USB port code (create with requested
//! names, then remove) outside the UI and prints each result.
//!
//! cargo run -p serialloom-core --example usb_probe -- COM30 COM31

#[cfg(windows)]
fn main() {
    use serialloom_core::driver::PortDriver;
    use serialloom_core::driver::native::{ADMIN_FLAG, admin_main};
    use serialloom_core::driver::usb::UsbDriver;

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(ADMIN_FLAG) {
        std::process::exit(admin_main(&args[1..]));
    }
    serialloom_core::logging::init("usb-probe");

    let dir = std::env::temp_dir().join(format!("serialloom-usb-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let driver = UsbDriver::new(&dir);
    println!("driver: {:?}", driver.info());

    let names: Vec<String> = if args.is_empty() {
        vec!["COM30".into(), "COM31".into()]
    } else {
        args
    };
    println!("creating {names:?} ...");
    match driver.create_ports(&names) {
        Ok(ports) => {
            for p in &ports {
                println!("  created {} state={:?} problem={:?}", p.name, p.state, p.problem);
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
            let created: Vec<String> = ports.iter().map(|p| p.name.clone()).collect();
            println!("removing {created:?} ...");
            match driver.remove_ports(&created) {
                Ok(()) => println!("  removed"),
                Err(e) => println!("  remove failed: {e}"),
            }
        }
        Err(e) => println!("  create failed: {e}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(windows))]
fn main() {}
