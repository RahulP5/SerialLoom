// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // When relaunched through UAC to create or remove virtual ports, run that
    // one operation and exit without starting the UI.
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some(serialloom_core::driver::native::ADMIN_FLAG) {
            serialloom_core::logging::init("admin");
            std::process::exit(serialloom_core::driver::native::admin_main(&args[1..]));
        }
    }
    serialloom_lib::run()
}
