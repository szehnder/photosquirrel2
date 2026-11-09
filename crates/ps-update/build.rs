// Release builds are marked by `cargo xtask package` (ADR-0127): rebuild when the marker changes, and when the
// public update keys the binary embeds change (ADR-0125).
fn main() {
    println!("cargo:rerun-if-env-changed=PHOTOSQUIRREL_RELEASE_BUILD");
    println!("cargo:rerun-if-changed=../../packaging/update-key.pub");
    println!("cargo:rerun-if-changed=../../packaging/update-key-next.pub");
}
