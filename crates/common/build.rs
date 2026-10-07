fn main() {
    // sqlx::migrate! membaca folder migrations saat compile; tanpa ini perubahan migrasi tidak memicu rebuild.
    println!("cargo:rerun-if-changed=migrations");
}
