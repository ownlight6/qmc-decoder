// On Windows, hide the console window when launching as a GUI app.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

fn main() {
    qmc_decoder_app_lib::run()
}