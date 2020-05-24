use http::Method;
use http::{Request, Response};
use hyper::Body;
use mpd::Client;
use reset_router::RequestExtensions;
use reset_router::Router;
use std::error::Error;
use std::net;
use std::ops::Add;
use std::ops::Sub;
use std::sync::Arc;
use std::time;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Mutex;
use tokio::time::timeout;

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

#[derive(Clone)]
struct Context {
    tx: Arc<Mutex<Sender<SleepMessage>>>,
}

async fn start_handler(req: Request<Body>) -> Result<Response<Body>, Response<Body>> {
    let state = req.data::<Context>().unwrap();
    let (dur,) = req
        .parsed_captures::<(u64,)>()
        .map_err(|_| Response::builder().body("Seconds missing".into()).unwrap())?;
    let seconds = std::time::Duration::from_secs(dur);
    let res = Response::builder()
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(format!("Sleeping for {:?} seconds…\n", dur).into())
        .unwrap();

    state
        .tx
        .lock()
        .await
        .send(SleepMessage::StartTimer(seconds))
        .await
        .unwrap();
    Ok(res)
}

async fn cancel_handler(req: Request<Body>) -> Result<Response<Body>, Response<Body>> {
    let state = req.data::<Context>().unwrap();
    let res = Response::builder()
        .header("Content-Type", "text/plain; charset=utf-8")
        .body("Canceling sleep timer…\n".into())
        .unwrap();

    state
        .tx
        .lock()
        .await
        .send(SleepMessage::Cancel)
        .await
        .unwrap();
    Ok(res)
}

async fn status_handler(_: Request<Body>) -> Result<Response<Body>, Response<Body>> {
    Ok(Response::builder()
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(format!("{:?}\n", get_status()).into())
        .unwrap())
}

async fn pause_handler(_: Request<Body>) -> Result<Response<Body>, Response<Body>> {
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

pub enum RecvTimeoutError {
    Timeout,
    Disconnected,
}

async fn run_mpd_handler(mut rx: Receiver<SleepMessage>) {
    let mut state = Option::None::<SleepTimerState>;
    loop {
        let message = match state {
            Option::None => rx
                .recv()
                .await
                .ok_or_else(|| RecvTimeoutError::Disconnected),
            Option::Some(state) => {
                let now = time::Instant::now();

                let timeout_duration = if now.sub(state.start) > state.duration {
                    time::Duration::new(0, 0)
                } else {
                    state.start.add(state.duration).sub(now)
                };
                timeout(timeout_duration, rx.recv()).await.map_or_else(
                    |_| Err(RecvTimeoutError::Timeout),
                    |s| s.ok_or_else(|| RecvTimeoutError::Timeout),
                )
            }
        };
        state = match message {
            Err(e) => match e {
                RecvTimeoutError::Timeout => {
                    match connect_mpd().and_then(|mut conn| sleep_now(&mut conn)) {
                        Ok(_) => {}
                        Err(e) => println!("Failed to connnect: {:?}", e),
                    };

                    Option::None
                }
                RecvTimeoutError::Disconnected => return,
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
}

pub async fn run() -> Result<(), Box<dyn Error>> {
    let (tx, rx) = channel::<SleepMessage>(100);

    tokio::spawn(run_mpd_handler(rx));

    let state = Context {
        tx: Arc::new(Mutex::new(tx)),
    };

    println!("Listening on {}:{}", LISTEN_IP, LISTEN_PORT);
    let router = Router::build()
        .data(state)
        .add(Method::GET, r"/sleep/start/(\d+)", start_handler)
        .add(Method::GET, r"/sleep/cancel", cancel_handler)
        .add(Method::GET, r"/pause", pause_handler)
        .add(Method::GET, r"/sleep/status", status_handler)
        .add_not_found(|_| async {
            Result::<Response<Body>, Response<Body>>::Ok(
                Response::builder()
                    .body("Route not found\n".into())
                    .unwrap(),
            )
        })
        .finish()?;

    let addr = net::SocketAddr::new(LISTEN_IP.parse()?, LISTEN_PORT);
    let server = hyper::Server::bind(&addr).serve(router);

    server.await?;

    Ok(())
}
