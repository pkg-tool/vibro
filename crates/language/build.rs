fn main() {
    if let Ok(bundled) = std::env::var("VECTOR_BUNDLE") {
        println!("cargo:rustc-env=VECTOR_BUNDLE={}", bundled);
    }
}
