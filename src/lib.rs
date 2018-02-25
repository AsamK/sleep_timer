extern crate hyper;
extern crate mime;
extern crate mpd;
extern crate num_cpus;
extern crate reset_router;
extern crate tokio_core;

use hyper::header::ContentType;
use hyper::server::Response;
use mpd::Client;
use reset_router::hyper::Context;
use reset_router::hyper::ext::ServiceExtensions;
use reset_router::hyper::Router;
use std::error::Error;
use std::net;
use std::ops::Add;
use std::ops::Sub;
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
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

fn get_status() -> Result<mpd::Status, Box<Error>> {
    let status = connect_mpd()?.status()?;
    Ok(status)
}

fn status_handler(_: Context) -> Result<Response, Response> {
    Ok(Response::new()
        .with_header(ContentType(mime::TEXT_PLAIN_UTF_8))
        .with_body(format!("{:?}\n", get_status())))
}

fn pause_handler(_: Context) -> Result<Response, Response> {
    let mut conn = connect_mpd().unwrap();
    conn.toggle_pause().expect("Failed to send pause command");
    let state = conn.status().expect("Failed to send status command").state;

    Ok(Response::new()
        .with_header(ContentType(mime::TEXT_PLAIN_UTF_8))
        .with_body(format!("State is now {:?}!\n", state)))
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

pub fn run() -> Result<(), Box<Error>> {
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
        .add_get(r"/sleep/start/(\d+)", move |ctx: Context| {
            let dur = match ctx.extract_captures() {
                Ok((seconds,)) => std::time::Duration::from_secs(seconds),
                Err(_) => return Err(Response::new().with_body("Seconds missing")),
            };
            let res = Response::new()
                .with_header(ContentType(mime::TEXT_PLAIN_UTF_8))
                .with_body(format!("Sleeping for {:?} seconds…\n", dur));

            tx_mux2
                .lock()
                .unwrap()
                .send(SleepMessage::StartTimer(dur))
                .unwrap();
            Ok(res)
        })
        .add_get(r"/sleep/cancel", move |_: Context| {
            let res = Response::new()
                .with_header(ContentType(mime::TEXT_PLAIN_UTF_8))
                .with_body(format!("Canceling sleep timer…\n"));

            tx_mux.lock().unwrap().send(SleepMessage::Cancel).unwrap();
            Ok::<_, Response>(res)
        })
        .add_get(r"/pause", pause_handler)
        .add_get(r"/sleep/status", status_handler)
        .add_not_found(|_| Ok::<_, Response>(Response::new().with_body("Route not found\n")))
        .finish()?;

    router
        .quick_serve(
            num_cpus::get(),
            net::SocketAddr::new(LISTEN_IP.parse().unwrap(), LISTEN_PORT),
            || ::tokio_core::reactor::Core::new().unwrap(),
        )?;
    Ok(())
}