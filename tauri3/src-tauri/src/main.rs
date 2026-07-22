#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(all(not(debug_assertions), not(feature = "custom-protocol")))]
compile_error!("DH BOT release builds must enable the custom-protocol feature");

fn main() {
    dh_bot_lib::run();
}
