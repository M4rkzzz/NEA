fn main() {
    println!("cargo:rerun-if-changed=../dist");
    let frontend_entrypoint = std::path::Path::new("../dist/index.html");
    if !frontend_entrypoint.is_file() {
        panic!("frontend build is missing ../dist/index.html");
    }
    tauri_build::build()
}
