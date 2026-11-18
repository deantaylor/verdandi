#[cfg(target_os = "macos")]
mod button_lights;
mod capture;
mod config;
mod controls;
mod framebuffer;
#[cfg(target_os = "macos")]
mod midi_buttons;
mod output;

use capture::Capture;
use clap::Parser;
use config::Cli;
use framebuffer::CapturedFrame;
use output::EncdrOutput;
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

static RUNNING: AtomicBool = AtomicBool::new(true);

extern "C" fn stop(_: libc::c_int) {
    RUNNING.store(false, Ordering::Relaxed);
}

fn main() -> Result<(), String> {
    let mut cli = Cli::parse();
    cli.apply_config()?;
    let path = cli.socket.unwrap_or_else(capture::default_path);
    if cli.print_socket {
        println!("{}", path.display());
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let target = cli.target.unwrap_or_default();
    if cli.list_devices {
        EncdrOutput::new(target, cli.device, cli.usb, false)?.list();
        return Ok(());
    }
    unsafe {
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
    }
    let mut capture = Capture::bind(path.clone())
        .map_err(|error| format!("cannot listen on {}: {error}", path.display()))?;
    eprintln!("Waiting for patched Verdandi on {}", path.display());

    if let Some(snapshot) = cli.snapshot {
        while RUNNING.load(Ordering::Relaxed) {
            if let Some(frame) = capture.poll().map_err(|error| error.to_string())? {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&snapshot)
                    .map_err(|error| format!("{}: {error}", snapshot.display()))?;
                write!(file, "P6\n{} {}\n255\n", frame.width, frame.height)
                    .and_then(|()| file.write_all(&frame.rgb))
                    .map_err(|error| format!("{}: {error}", snapshot.display()))?;
                eprintln!("Saved native OLED frame to {}", snapshot.display());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        return Ok(());
    }

    let mut output = EncdrOutput::new(target, cli.device, cli.usb, !cli.display_only)?;
    let frame = Arc::new(Mutex::new(None::<Arc<CapturedFrame>>));
    let output_frame = Arc::clone(&frame);
    let period = Duration::from_secs_f64(1.0 / f64::from(cli.fps.unwrap_or(30)));
    let worker = std::thread::Builder::new()
        .name("verdandi-ni-output".into())
        .spawn(move || {
            let mut frame_due = Instant::now();
            while RUNNING.load(Ordering::Relaxed) {
                let start = Instant::now();
                let submit = start >= frame_due;
                if submit {
                    frame_due = start + period;
                }
                output.update(&output_frame, submit);
                let now = Instant::now();
                let next_poll = (start + Duration::from_millis(8)).min(frame_due);
                std::thread::sleep(next_poll.saturating_duration_since(now));
            }
        })
        .map_err(|error| error.to_string())?;

    let capture_result = read_frames(&mut capture, &frame);
    RUNNING.store(false, Ordering::Relaxed);
    worker
        .join()
        .map_err(|_| "display output thread stopped unexpectedly".to_string())?;
    capture_result
}

fn read_frames(
    capture: &mut Capture,
    latest: &Mutex<Option<Arc<CapturedFrame>>>,
) -> Result<(), String> {
    let mut was_live = false;
    while RUNNING.load(Ordering::Relaxed) {
        if let Some(frame) = capture.poll().map_err(|error| error.to_string())? {
            let mut latest = latest.lock().unwrap();
            if latest.as_deref() != Some(&frame) {
                *latest = Some(Arc::new(frame));
            }
        }
        let live = capture.is_live();
        if !was_live && live {
            eprintln!("Receiving Verdandi OLED frames.");
        } else if was_live && !live {
            *latest.lock().unwrap() = None;
            eprintln!("Verdandi frame stream paused; waiting for it to return.");
        }
        was_live = live;
        std::thread::sleep(Duration::from_millis(8));
    }
    Ok(())
}
