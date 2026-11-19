fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        println!("cargo:rustc-link-arg=-T{dir}/linker-{arch}.ld");
        println!("cargo:rustc-link-arg=--no-dynamic-linker");
        println!("cargo:rustc-link-arg=-zmax-page-size=0x1000");
    }
    println!("cargo:rerun-if-changed=linker-{arch}.ld");
}
