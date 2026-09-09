fn main() {
    #[cfg(target_os = "windows")]
    winresource::WindowsResource::new()
        .set_icon("../../assets/whisperia-server.ico")
        .compile()
        .expect("the icon goes into the exe");
    println!("cargo:rerun-if-changed=../../assets/whisperia-server.ico");
}
