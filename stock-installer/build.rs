fn main() {
    embuild::espidf::sysenv::output();
    println!("cargo:rerun-if-changed=payload.bin");
}
