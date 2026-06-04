fn main() {
    cc::Build::new()
        .file("crash_guard.c")
        .compile("crash_guard");
    tauri_build::build()
}
