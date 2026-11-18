use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Target {
    #[default]
    Left,
    Right,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Mirror Verdandi's native OLED frames onto one NI display"
)]
pub struct Cli {
    /// Unix socket path. The default is in your macOS temporary directory.
    #[arg(long)]
    pub socket: Option<PathBuf>,

    /// Display to mirror (default: left).
    #[arg(long, value_enum)]
    pub target: Option<Target>,

    /// Controller model substring or exact USB serial.
    #[arg(long)]
    pub device: Option<String>,

    /// Maximum display updates per second (default: 30).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=60))]
    pub fps: Option<u32>,

    /// Read settings from a key=value file.
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Print the socket path to use for VERDANDI_FRAME_SOCKET and exit.
    #[arg(long, conflicts_with_all = ["list_devices", "snapshot", "usb"])]
    pub print_socket: bool,

    /// List connected eligible Encdr devices and exit.
    #[arg(long, conflicts_with_all = ["snapshot", "usb"])]
    pub list_devices: bool,

    /// Save one native OLED frame as PPM and exit without opening the NI device.
    #[arg(long, conflicts_with = "usb")]
    pub snapshot: Option<PathBuf>,

    /// Use raw USB for standalone operation without NI instance switching.
    #[arg(long)]
    pub usb: bool,

    /// Mirror the display without forwarding controller buttons or knobs.
    #[arg(long)]
    pub display_only: bool,
}

#[derive(Default)]
struct FileConfig {
    socket: Option<PathBuf>,
    target: Option<Target>,
    device: Option<String>,
    fps: Option<u32>,
}

impl Cli {
    pub fn apply_config(&mut self) -> Result<(), String> {
        if let Some(path) = &self.config {
            let config = load(path)?;
            self.socket = self.socket.take().or(config.socket);
            self.target = self.target.or(config.target);
            self.device = self.device.take().or(config.device);
            self.fps = self.fps.or(config.fps);
        }
        Ok(())
    }
}

fn load(path: &Path) -> Result<FileConfig, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut config = FileConfig::default();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let invalid = |message: &str| format!("{}:{}: {message}", path.display(), index + 1);
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| invalid("expected key=value"))?;
        let value = value.trim().trim_matches('"');
        if value.is_empty() {
            return Err(invalid("expected a value"));
        }
        match key.trim() {
            "socket" => config.socket = Some(value.into()),
            "target" => {
                config.target = Some(
                    Target::from_str(value, false)
                        .map_err(|_| invalid("target must be left or right"))?,
                )
            }
            "device" => config.device = Some(value.into()),
            "fps" => {
                config.fps = Some(
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|fps| (1..=60).contains(fps))
                        .ok_or_else(|| invalid("fps must be between 1 and 60"))?,
                )
            }
            _ => return Err(invalid("unknown setting")),
        }
    }
    Ok(config)
}
