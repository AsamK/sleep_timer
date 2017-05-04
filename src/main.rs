extern crate mpd;
extern crate hyper;
extern crate reroute;

use mpd::Client;
use hyper::Server;
use hyper::server::{Request, Response};
use reroute::{Captures, RouterBuilder};
use std::io::Write;

const MPD_IP: &str = "127.0.0.1";
const MPD_PORT: u16 = 6600;

const LISTEN_IP: &str = "0.0.0.0";
const LISTEN_PORT: u16 = 5613;

fn connect_mpd() -> Client {
    return Client::connect((MPD_IP, MPD_PORT)).expect("Failed to connect to mpd")
}

fn send_command() {
    let mut conn = connect_mpd();
    conn.pause(true).expect("Failed to send pause command");
    conn.volume(80).expect("Failed to send volume command");
    println!("Current volume: {}", conn.status().unwrap().volume);
    println!("Current volume: {:?}", conn.status());
}

fn digit_handler(_: Request, res: Response, c: Captures) {
    println!("captures: {:?}", c);
    res.send(b"It works for digits!\n").unwrap();
    send_command();
}

fn pause_handler(_: Request, res: Response, _: Captures) {
    let mut conn = connect_mpd();
    conn.toggle_pause().expect("Failed to send pause command");
    let state = conn.status().expect("Failed to send status command").state;
    res.send(format!("State is now {:?}!\n", state).as_bytes()).unwrap();
}

fn sleep_handler(_: Request, res: Response, c: Captures) {
    let dur = match c {
        Some(cap) => std::time::Duration::from_secs(cap[1].parse::<u64>().unwrap()),
        None => return,
    };
    let mut stream = res.start().unwrap();

    stream.write_all(format!("Sleeping for {:?} seconds…\n", dur).as_bytes()).unwrap();
    stream.flush().unwrap();

    std::thread::sleep(dur);

    stream.write_all(b"Going to sleep now!\n").unwrap();
    stream.flush().unwrap();

    let mut conn = connect_mpd();
    let volume = conn.status().expect("Failed to send status command").volume;
    for i in (40..volume).rev() {
        conn.volume(i).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    conn.pause(true).expect("Failed to send pause command");
    conn.volume(volume).unwrap();
    let status = conn.status().expect("Failed to send status command");
    stream.write_all(format!("State is now {:?}, volume {}!\n", status.state, status.volume).as_bytes()).unwrap();
    stream.end().unwrap();
}

fn main() {
    println!("Starting mpd sleep timer server!");
    println!("Listening on {}:{}", LISTEN_IP, LISTEN_PORT);
    let mut router_builder = RouterBuilder::new();

    // Use raw strings so you don't need to escape patterns.
    router_builder.get(r"/sleep/start/(\d+)", sleep_handler);
    router_builder.get(r"/sleep/status/(\d+)", digit_handler);
    router_builder.get(r"/pause", pause_handler);
    router_builder.not_found(|_, res, _| {res.send(b"LaLaLa").unwrap();});

    let router = router_builder.finalize().unwrap();

    // You can pass the router to hyper's Server's handle function as it
    // implements the Handle trait.
    Server::http((LISTEN_IP, LISTEN_PORT)).unwrap().handle(router).unwrap();
}
