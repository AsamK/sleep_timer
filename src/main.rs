use std::process;

mod lib;

#[tokio::main]
async fn main() {
    println!("Starting mpd sleep timer server!");

    if let Err(e) = lib::run().await {
        println!("Application error: {}", e);
        process::exit(1);
    }
}
