fn main() {
    // `sqlx::migrate!` embeds the files at compile time and cargo does not track them by itself.
    println!("cargo:rerun-if-changed=migrations");
}
