extern crate futures;
extern crate http;
extern crate hyper;
extern crate mime;
extern crate mpd;
extern crate num_cpus;
extern crate reset_router;
extern crate tokio_core;

use crate::hyper::rt::Future;
use http::{Request, Response};
use hyper::Body;
use mpd::Client;
use reset_router::bits::Method;
use reset_router::RequestExtensions;
use reset_router::Router;
use std::error::Error;
use std::net;
use std::ops::Add;
use std::ops::Sub;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time;

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

fn get_status() -> Result<mpd::Status, Box<dyn Error>> {
    let status = connect_mpd()?.status()?;
    Ok(status)
}

fn status_handler(_: Request<Body>) -> Result<Response<Body>, Response<Body>> {
    Ok(Response::builder()
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(format!("{:?}\n", get_status()).into())
        .unwrap())
}

fn pause_handler(_: Request<Body>) -> Result<Response<Body>, Response<Body>> {
    let mut conn = connect_mpd().unwrap();
    conn.toggle_pause().expect("Failed to send pause command");
    let state = conn.status().expect("Failed to send status command").state;

    Ok(Response::builder()
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(format!("State is now {:?}!\n", state).into())
        .unwrap())
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

pub fn run() -> Result<(), Box<dyn Error>> {
    let (tx, rx) = mpsc::channel::<SleepMessage>();
    thread::spawn(move || {
        let mut state = Option::None::<SleepTimerState>;
        loop {
            let message = match state {
                Option::None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
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
                Err(e) => match e {
                    mpsc::RecvTimeoutError::Timeout => {
                        match connect_mpd().and_then(|mut conn| sleep_now(&mut conn)) {
                            Ok(_) => {}
                            Err(e) => println!("Failed to connnect: {:?}", e),
                        };

                        Option::None
                    }
                    mpsc::RecvTimeoutError::Disconnected => return,
                },
                Ok(message) => match message {
                    SleepMessage::StartTimer(new_duration) => Option::from(SleepTimerState {
                        duration: new_duration,
                        start: time::Instant::now(),
                    }),
                    SleepMessage::Cancel => Option::None,
                },
            };
        }
    });

    let tx_mux = Arc::new(Mutex::new(tx));
    let tx_mux2 = tx_mux.clone();

    println!("Listening on {}:{}", LISTEN_IP, LISTEN_PORT);
    let router = Router::build()
        .add(
            Method::GET,
            r"/sleep/start/(\d+)",
            move |req: Request<Body>| -> Result<Response<Body>, Response<Body>> {
                let (dur,) = req
                    .parsed_captures::<(u64,)>()
                    .map_err(|_| Response::builder().body("Seconds missing".into()).unwrap())?;
                let seconds = std::time::Duration::from_secs(dur);
                let res = Response::builder()
                    .header("Content-Type", "text/plain; charset=utf-8")
                    .body(format!("Sleeping for {:?} seconds…\n", dur).into())
                    .unwrap();

                tx_mux2
                    .lock()
                    .unwrap()
                    .send(SleepMessage::StartTimer(seconds))
                    .unwrap();
                Ok(res)
            },
        )
        .add(
            Method::GET,
            r"/sleep/cancel",
            move |_: Request<Body>| -> Result<Response<Body>, Response<Body>> {
                let res = Response::builder()
                    .header("Content-Type", "text/plain; charset=utf-8")
                    .body("Canceling sleep timer…\n".into())
                    .unwrap();

                tx_mux.lock().unwrap().send(SleepMessage::Cancel).unwrap();
                Ok(res)
            },
        )
        .add(Method::GET, r"/pause", pause_handler)
        .add(Method::GET, r"/sleep/status", status_handler)
        .add_not_found(|_| -> Result<Response<Body>, Response<Body>> {
            Ok(Response::builder()
                .body("Route not found\n".into())
                .unwrap())
        })
        .finish()
        .unwrap();

    let addr = net::SocketAddr::new(LISTEN_IP.parse().unwrap(), LISTEN_PORT);
    let server = hyper::Server::bind(&addr)
        .serve(router)
        .map_err(|e| eprintln!("error: {}", e));

    hyper::rt::run(server);

    Ok(())
}
