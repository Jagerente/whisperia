fn main() {
    #[cfg(target_os = "windows")]
    winresource::WindowsResource::new()
        .set_icon("../../assets/whisperia.ico")
        .compile()
        .expect("the icon goes into the exe");
    println!("cargo:rerun-if-changed=../../assets/whisperia.ico");

    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_debug_info(std::env::var_os("CARGO_FEATURE_TESTING").is_some());
    slint_build::compile_with_config("ui/app.slint", config).expect("the window's markup compiles");
}
