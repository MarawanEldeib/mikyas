// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // First, before anything can load a DLL.
    mikyas_lib::restrict_dll_search();
    mikyas_lib::run()
}
