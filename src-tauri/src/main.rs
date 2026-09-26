// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // First, before anything can load a DLL.
    cuw_widget_lib::restrict_dll_search();
    cuw_widget_lib::run()
}
