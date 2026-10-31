// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    farmlab_app_lib::early_limits();
    farmlab_app_lib::run();
}
