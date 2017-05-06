extern crate mpd;
extern crate hyper;
extern crate reroute;

use mpd::Client;
use hyper::Server;
use hyper::server::{Request, Response};
use hyper::header::ContentType;
use hyper::mime::{Mime, TopLevel, SubLevel, Attr, Value};
use reroute::{Captures, RouterBuilder};
use std::time;
use std::error::Error;
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread::spawn;
use std::ops::Add;
use std::ops::Sub;

const MPD_IP: &str = "127.0.0.1";
const MPD_PORT: u16 = 6600;

const LISTEN_IP: &str = "0.0.0.0";
const LISTEN_PORT: u16 = 5613;

#[derive(Debug)]
struct SleepTimerState {
    duration: time::Duration,
    start: time::Instant,
}

#[derive(Debug)]
enum SleepMessage {
    StartTimer(time::Duration),
    Cancel,
}

fn connect_mpd() -> mpd::error::Result<Client> {
    Client::connect((MPD_IP, MPD_PORT))
}

fn get_status() -> Result<mpd::Status, Box<Error>> {
    let status = connect_mpd()?.status()?;
    Ok(status)
}

fn status_handler(_: Request, mut res: Response, _: Captures) {
    res.headers_mut()
        .set(ContentType(Mime(TopLevel::Text,
                              SubLevel::Plain,
                              vec![(Attr::Charset, Value::Utf8)])));
    res.send(format!("{:?}", get_status()).as_bytes())
        .unwrap();
}

fn pause_handler(_: Request, res: Response, _: Captures) {
    let mut conn = connect_mpd().unwrap();
    conn.toggle_pause()
        .expect("Failed to send pause command");
    let state = conn.status()
        .expect("Failed to send status command")
        .state;
    res.send(format!("State is now {:?}!\n", state).as_bytes())
        .unwrap();
}

fn sleep_now(conn: &mut mpd::Client) -> mpd::error::Result<()> {
    let volume = conn.status()?.volume;
    for i in (40..volume).rev() {
        conn.volume(i)?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    conn.pause(true)?;
    conn.volume(volume)?;
    Ok(())
}


fn main() {
    println!("Starting mpd sleep timer server!");

    let (tx, rx) = mpsc::channel::<SleepMessage>();
    spawn(move || {
        let mut state = Option::None::<SleepTimerState>;
        loop {
            let message = match state {
                Option::None => {
                    rx.recv()
                        .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                }
                Option::Some(state) => {
                    let now = time::Instant::now();

                    let timeout = if now.sub(state.start) > state.duration {
                        time::Duration::new(0, 0)
                    } else {
                        state.start.add(state.duration).sub(now)
                    };
                    rx.recv_timeout(timeout)
                }
            };
            state = match message {
                Err(e) => {
                    match e {
                        mpsc::RecvTimeoutError::Timeout => {
                            match connect_mpd().and_then(|mut conn| sleep_now(&mut conn)) {
                                Ok(_) => {}
                                Err(e) => println!("Failed to connnect: {:?}", e),
                            };

                            Option::None
                        }
                        mpsc::RecvTimeoutError::Disconnected => return,
                    }
                }
                Ok(message) => {
                    match message {
                        SleepMessage::StartTimer(new_duration) => {
                            Option::from(SleepTimerState {
                                             duration: new_duration,
                                             start: time::Instant::now(),
                                         })
                        }
                        SleepMessage::Cancel => Option::None,
                    }
                }
            };
        }
    });

    println!("Listening on {}:{}", LISTEN_IP, LISTEN_PORT);
    let mut router_builder = RouterBuilder::new();
    let tx_mux = Mutex::new(tx);
    // Use raw strings so you don't need to escape patterns.
    router_builder.get(r"/sleep/start/(\d+)",
                       move |_: Request, mut res: Response, c: Captures| {
        let dur = match c {
            Some(cap) => std::time::Duration::from_secs(cap[1].parse::<u64>().unwrap()),
            None => return,
        };
        res.headers_mut()
            .set(ContentType(Mime(TopLevel::Text,
                                  SubLevel::Plain,
                                  vec![(Attr::Charset, Value::Utf8)])));
        res.send(format!("Sleeping for {:?} seconds…\n", dur).as_bytes())
            .unwrap();

        tx_mux
            .lock()
            .unwrap()
            .send(SleepMessage::StartTimer(dur))
            .unwrap();
    });
    router_builder.get(r"/sleep/status", status_handler);
    router_builder.get(r"/pause", pause_handler);
    router_builder.not_found(|_, res, _| { res.send(b"LaLaLa").unwrap(); });

    let router = router_builder.finalize().unwrap();

    // You can pass the router to hyper's Server's handle function as it
    // implements the Handle trait.
    Server::http((LISTEN_IP, LISTEN_PORT))
        .unwrap()
        .handle(router)
        .unwrap();
}
