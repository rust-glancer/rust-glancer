fn main() {
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    std::fs::write(output.join("generated.rs"), "pub fn generated_value() -> u16 { 42 }\n")
        .expect("Cargo makes OUT_DIR writable");
    println!("cargo::rerun-if-changed=build.rs");
}
