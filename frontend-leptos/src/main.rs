mod account;
mod api;
mod app;
mod components;
mod detections;
mod doorbell;
mod playback;
mod ws;
mod cctv;

use app::App;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount_to_body(App);
}
