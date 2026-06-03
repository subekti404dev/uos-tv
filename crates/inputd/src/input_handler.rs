//! Input device handler with evdev access.
//!
//! On Linux: scans /dev/input/event* devices via evdev crate,
//! dispatches key events, and maps IR remote buttons.

use std::path::PathBuf;
use std::time::Duration;

/// Event from an input device.
#[derive(Debug, Clone)]
pub struct InputEvent {
    pub device: String,
    pub kind: EventKind,
}

#[derive(Debug, Clone)]
pub enum EventKind {
    KeyPress(u16),
    KeyRelease(u16),
    RemoteButton(RemoteKey),
}

/// Key codes untuk remote control UOS TV.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteKey {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    Home,
    Menu,
    VolumeUp,
    VolumeDown,
    Mute,
    Power,
    Source,
    ChannelUp,
    ChannelDown,
    Red,
    Green,
    Blue,
    Yellow,
    Play,
    Pause,
    Stop,
    Forward,
    Rewind,
    Numeric(u8),
}

pub struct InputHandler {
    devices: Vec<PathBuf>,
}

impl InputHandler {
    pub fn new() -> Self {
        let devices = Self::scan_devices();
        if !devices.is_empty() {
            tracing::info!(
                "Found {} input device(s): {}",
                devices.len(),
                devices
                    .iter()
                    .map(|d| d.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        } else {
            tracing::warn!("No /dev/input/event* devices found — input system idle");
        }
        Self { devices }
    }

    /// Blocking read loop — runs in a dedicated thread.
    /// Sends InputEvent via channel to the async dispatch loop.
    #[cfg(target_os = "linux")]
    pub fn read_loop(
        &self,
        event_tx: std::sync::mpsc::Sender<InputEvent>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use evdev::{Device, InputEventKind};

        if self.devices.is_empty() {
            std::thread::sleep(Duration::from_secs(5));
            return Ok(());
        }

        let mut opened: Vec<(PathBuf, evdev::Device)> = Vec::new();
        for path in &self.devices {
            match Device::open(path) {
                Ok(dev) => opened.push((path.clone(), dev)),
                Err(e) => tracing::warn!("Cannot open {}: {}", path.display(), e),
            }
        }

        loop {
            for (path, dev) in &mut opened {
                match dev.fetch_events() {
                    Ok(events) => {
                        let events: Vec<_> = events.collect();
                        for ev in &events {
                            let input_ev = Self::translate_ev(path, ev);
                            if let Some(ev) = input_ev {
                                if event_tx.send(ev).is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => continue,
                    Err(e) => {
                        tracing::debug!("evdev read error on {}: {}", path.display(), e);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(target_os = "linux")]
    fn translate_ev(path: &PathBuf, ev: &evdev::InputEvent) -> Option<InputEvent> {
        let kind = match ev.kind() {
            evdev::InputEventKind::Key(key) => {
                let code = key.code();
                if let Some(remote_key) = IrRemoteMap::decode(code) {
                    EventKind::RemoteButton(remote_key)
                } else if ev.value() == 1 {
                    EventKind::KeyPress(code)
                } else if ev.value() == 0 {
                    EventKind::KeyRelease(code)
                } else {
                    return None;
                }
            }
            _ => return None,
        };

        Some(InputEvent {
            device: path.display().to_string(),
            kind,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn read_loop(
        &self,
        _event_tx: std::sync::mpsc::Sender<InputEvent>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    #[cfg(target_os = "linux")]
    fn scan_devices() -> Vec<PathBuf> {
        let mut devices = Vec::new();
        let dir = match std::fs::read_dir("/dev/input") {
            Ok(d) => d,
            Err(_) => return devices,
        };
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("event") {
                let path = entry.path();
                if let Ok(dev) = evdev::Device::open(&path) {
                    let dev_name = dev.name().unwrap_or("unknown").to_string();
                    tracing::debug!("  input: {} → {}", path.display(), dev_name);
                    devices.push(path);
                }
            }
        }
        devices
    }

    #[cfg(not(target_os = "linux"))]
    fn scan_devices() -> Vec<PathBuf> {
        Vec::new()
    }
}

/// IR remote protocol decoder.
/// Maps common Linux input key codes to UOS RemoteKey.
pub struct IrRemoteMap;

impl IrRemoteMap {
    pub fn decode(code: u16) -> Option<RemoteKey> {
        use RemoteKey::*;
        Some(match code {
            103 => Up,
            108 => Down,
            105 => Left,
            106 => Right,
            28 => Ok,
            158 => Back,
            102 => Home,
            139 => Menu,
            115 => VolumeUp,
            114 => VolumeDown,
            113 => Mute,
            116 => Power,
            142 => Power,
            362 => Source,
            402 => ChannelUp,
            403 => ChannelDown,
            398 => Red,
            399 => Green,
            400 => Blue,
            401 => Yellow,
            207 => Play,
            119 => Pause,
            128 => Stop,
            208 => Forward,
            168 => Rewind,
            2 => Numeric(1),
            3 => Numeric(2),
            4 => Numeric(3),
            5 => Numeric(4),
            6 => Numeric(5),
            7 => Numeric(6),
            8 => Numeric(7),
            9 => Numeric(8),
            10 => Numeric(9),
            11 => Numeric(0),
            _ => return None,
        })
    }

    pub fn name(key: &RemoteKey) -> &'static str {
        match key {
            RemoteKey::Up => "Up",
            RemoteKey::Down => "Down",
            RemoteKey::Left => "Left",
            RemoteKey::Right => "Right",
            RemoteKey::Ok => "OK",
            RemoteKey::Back => "Back",
            RemoteKey::Home => "Home",
            RemoteKey::Menu => "Menu",
            RemoteKey::VolumeUp => "Vol+",
            RemoteKey::VolumeDown => "Vol-",
            RemoteKey::Mute => "Mute",
            RemoteKey::Power => "Power",
            RemoteKey::Source => "Source",
            RemoteKey::ChannelUp => "CH+",
            RemoteKey::ChannelDown => "CH-",
            RemoteKey::Red => "Red",
            RemoteKey::Green => "Green",
            RemoteKey::Blue => "Blue",
            RemoteKey::Yellow => "Yellow",
            RemoteKey::Play => "Play",
            RemoteKey::Pause => "Pause",
            RemoteKey::Stop => "Stop",
            RemoteKey::Forward => "Fwd",
            RemoteKey::Rewind => "Rwd",
            RemoteKey::Numeric(0) => "0",
            RemoteKey::Numeric(1) => "1",
            RemoteKey::Numeric(2) => "2",
            RemoteKey::Numeric(3) => "3",
            RemoteKey::Numeric(4) => "4",
            RemoteKey::Numeric(5) => "5",
            RemoteKey::Numeric(6) => "6",
            RemoteKey::Numeric(7) => "7",
            RemoteKey::Numeric(8) => "8",
            RemoteKey::Numeric(9) => "9",
            RemoteKey::Numeric(_) => "?",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_key_decoding() {
        assert_eq!(IrRemoteMap::decode(103), Some(RemoteKey::Up));
        assert_eq!(IrRemoteMap::decode(108), Some(RemoteKey::Down));
        assert_eq!(IrRemoteMap::decode(115), Some(RemoteKey::VolumeUp));
        assert_eq!(IrRemoteMap::decode(116), Some(RemoteKey::Power));
        assert_eq!(IrRemoteMap::decode(2), Some(RemoteKey::Numeric(1)));
        assert_eq!(IrRemoteMap::decode(11), Some(RemoteKey::Numeric(0)));
    }

    #[test]
    fn test_unknown_code() {
        assert_eq!(IrRemoteMap::decode(999), None);
    }

    #[test]
    fn test_remote_key_names() {
        assert_eq!(IrRemoteMap::name(&RemoteKey::Up), "Up");
        assert_eq!(IrRemoteMap::name(&RemoteKey::VolumeUp), "Vol+");
        assert_eq!(IrRemoteMap::name(&RemoteKey::Numeric(5)), "5");
    }
}
