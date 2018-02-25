extern crate sleep_timer;

use std::process;

fn main() {
    println!("Starting mpd sleep timer server!");

    if let Err(e) = sleep_timer::run() {
        println!("Application error: {}", e);
        process::exit(1);
    }
}
