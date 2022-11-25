use std::process;

#[tokio::main]
async fn main() {
    println!("Starting mpd sleep timer server!");

    if let Err(e) = sleep_timer::run().await {
        println!("Application error: {}", e);
        process::exit(1);
    }
}
