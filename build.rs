fn main() {
    println!("cargo:rerun-if-changed=ui/app.slint");
    println!("cargo:rerun-if-changed=ui/assets/logo.png");
    println!("cargo:rerun-if-changed=assets/app.ico");

    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let material_path = std::path::Path::new(&manifest_dir).join("material-1.1.0/material.slint");

    let config = slint_build::CompilerConfiguration::new().with_library_paths(
        std::collections::HashMap::from([("material".to_string(), material_path)]),
    );

    slint_build::compile_with_config("ui/app.slint", config).expect("Failed to compile Slint UI");

    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.set("ProductName", "MediaFire Mini");
        res.set(
            "FileDescription",
            "MediaFire Mini - Downloader cartelle e file MediaFire",
        );
        res.compile()
            .expect("Failed to embed Windows icon resource");
    }
}
