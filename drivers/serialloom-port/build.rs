//! Configures linking against the UMDF libraries from the WDK.

fn main() -> Result<(), wdk_build::ConfigError> {
    wdk_build::configure_wdk_binary_build()
}
