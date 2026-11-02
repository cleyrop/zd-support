fn main() {
    println!(
        "cargo:rustc-env=ZD_SUPPORT_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
