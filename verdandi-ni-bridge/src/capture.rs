use crate::framebuffer::CapturedFrame;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const MAGIC: &[u8; 4] = b"VDF1";
const HEADER_LEN: usize = 8;
const PACKET_LEN: usize = HEADER_LEN + 128 * 64 * 3;
const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

pub fn default_path() -> PathBuf {
    std::env::temp_dir().join("verdandi-ni/frame.sock")
}

pub struct Capture {
    socket: UnixDatagram,
    path: PathBuf,
    _lock: File,
    packet: [u8; PACKET_LEN + 1],
    last_received: Option<Instant>,
}

impl Capture {
    pub fn bind(path: PathBuf) -> io::Result<Self> {
        let parent = path
            .parent()
            .filter(|_| path.is_absolute())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "use an absolute socket path")
            })?;
        match fs::DirBuilder::new().mode(0o700).create(parent) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the socket needs a private directory owned by you (mode 0700)",
            ));
        }

        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path.with_extension("lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == -1 {
            let error = io::Error::last_os_error();
            return Err(io::Error::new(
                error.kind(),
                format!("cannot lock frame socket: {error}"),
            ));
        }

        match fs::symlink_metadata(&path) {
            Ok(info) if info.file_type().is_socket() => fs::remove_file(&path)?,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "socket path contains a file",
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let capture = Self {
            socket: UnixDatagram::bind(&path)?,
            path,
            _lock: lock,
            packet: [0; PACKET_LEN + 1],
            last_received: None,
        };
        capture.socket.set_nonblocking(true)?;
        // Darwin's default receive space is smaller than a single OLED frame.
        let bytes: libc::c_int = 64 * 1024;
        if unsafe {
            libc::setsockopt(
                capture.socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVBUF,
                (&bytes as *const libc::c_int).cast(),
                std::mem::size_of_val(&bytes) as libc::socklen_t,
            )
        } == -1
        {
            return Err(io::Error::last_os_error());
        }
        Ok(capture)
    }

    pub fn poll(&mut self) -> io::Result<Option<CapturedFrame>> {
        // Bound the work under a continuously writing sender.
        let mut latest = None;
        for _ in 0..32 {
            match self.socket.recv(&mut self.packet) {
                Ok(size) => {
                    if size == PACKET_LEN
                        && self.packet[..4] == *MAGIC
                        && self.packet[4..8] == [128, 0, 64, 0]
                    {
                        latest = Some(CapturedFrame {
                            width: 128,
                            height: 64,
                            rgb: self.packet[HEADER_LEN..size].into(),
                        });
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        if latest.is_some() {
            self.last_received = Some(Instant::now());
        }
        Ok(latest)
    }

    pub fn is_live(&self) -> bool {
        self.last_received
            .is_some_and(|last| last.elapsed() < FRAME_TIMEOUT)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_latest_valid_datagram_and_rebinds() {
        let parent = std::env::temp_dir().join(format!("vd-test-{}", std::process::id()));
        let path = parent.join("frame.sock");
        let mut capture = Capture::bind(path.clone()).unwrap();
        assert!(Capture::bind(path.clone()).is_err());
        let sender = UnixDatagram::unbound().unwrap();
        let buffer: libc::c_int = 32 * 1024;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    sender.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&buffer as *const libc::c_int).cast(),
                    std::mem::size_of_val(&buffer) as libc::socklen_t,
                )
            },
            0
        );

        let mut packet = [1; PACKET_LEN];
        packet[..8].copy_from_slice(b"VDF1\x80\x00\x40\x00");
        sender.send_to(&packet, &path).unwrap();
        packet[HEADER_LEN] = 2;
        sender.send_to(&packet, &path).unwrap();
        sender.send_to(b"invalid", &path).unwrap();
        assert_eq!(capture.poll().unwrap().unwrap().rgb[0], 2);
        assert!(capture.is_live());

        drop(capture);
        assert!(!path.exists());
        let mut capture = Capture::bind(path.clone()).unwrap();
        sender.send_to(&packet, &path).unwrap();
        assert!(capture.poll().unwrap().is_some());
        drop(capture);
        fs::remove_dir_all(parent).unwrap();
    }
}
