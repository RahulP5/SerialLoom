//! Diagnostic: creates a virtual pair through the Engine exactly like the
//! app's "Virtual pair" dialog, prints the result, then deletes it.
//!
//! cargo run -p serialloom-core --example engine_probe -- COM40 COM41

#[cfg(windows)]
fn main() {
    use serialloom_core::Engine;
    use serialloom_core::driver::native::{ADMIN_FLAG, admin_main};
    use serialloom_core::driver::usb::UsbDriver;
    use serialloom_core::model::{CreateHubRequest, EndpointRequest, HubKind};

    let args: Vec<String> = std::env::args().skip(1).collect();
    serialloom_core::logging::init("probe");
    if args.first().map(String::as_str) == Some(ADMIN_FLAG) {
        std::process::exit(admin_main(&args[1..]));
    }
    let a = args.first().cloned().unwrap_or_else(|| "COM40".into());
    let b = args.get(1).cloned().unwrap_or_else(|| "COM41".into());

    let dir = std::env::temp_dir().join(format!("serialloom-engine-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let engine = Engine::new(UsbDriver::new(&dir), None);

    let hub = engine.create_hub(CreateHubRequest {
        name: "Virtual null-modem".into(),
        kind: HubKind::Bridge,
        master: EndpointRequest::NewVirtual { name: a.clone() },
        members: vec![EndpointRequest::NewVirtual { name: b.clone() }],
        autostart: true,
        start: true,
    });
    match hub {
        Ok(h) => {
            println!(
                "asked {a}/{b}, got master={:?} member={:?}",
                h.spec.master.spec, h.spec.members[0].spec
            );
            for p in engine.virtual_ports().unwrap_or_default() {
                println!("  port {} state={:?} problem={:?}", p.name, p.state, p.problem);
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
            match engine.delete_hub(&h.spec.id, true) {
                Ok(()) => println!("deleted"),
                Err(e) => println!("delete failed: {e}"),
            }
        }
        Err(e) => println!("create failed: {e}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(windows))]
fn main() {}
