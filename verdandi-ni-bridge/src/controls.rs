use crate::config::Target;
use encdr::{DeviceDescriptor, Event};
use std::io;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use crate::midi_buttons::MidiButtons;
#[cfg(target_os = "macos")]
use encdr::usb::ni_service::protocol::InstanceInput;

const VERDANDI_OSC_PORT: u16 = 10111;
const ENCODER_TICKS_PER_STEP: i32 = 8;
#[cfg(target_os = "macos")]
const TAP_DURATION: Duration = Duration::from_millis(40);

#[derive(Clone, Copy)]
pub struct ControlMap {
    buttons: [&'static str; 3],
    encoders: [&'static str; 3],
    #[cfg(target_os = "macos")]
    instance_buttons: [u32; 3],
    #[cfg(target_os = "macos")]
    instance_encoders: [u32; 3],
}

impl ControlMap {
    pub fn for_device(desc: &DeviceDescriptor, target: Target) -> Option<Self> {
        if desc.vendor_id.0 != 0x17cc {
            return None;
        }
        let buttons = match target {
            Target::Left => ["top_1", "top_3", "top_4"],
            Target::Right => ["top_5", "top_7", "top_8"],
        };
        let encoders = match (desc.product_id.0, target) {
            (0x1600, Target::Left) => ["screen_encoder_1", "screen_encoder_3", "screen_encoder_4"],
            (0x1600, Target::Right) => ["screen_encoder_5", "screen_encoder_7", "screen_encoder_8"],
            (0x1610 | 0x1620 | 0x1630, Target::Left) => ["encoder_1", "encoder_3", "encoder_4"],
            (0x1610 | 0x1620 | 0x1630, Target::Right) => ["encoder_5", "encoder_7", "encoder_8"],
            _ => return None,
        };
        Some(Self {
            buttons,
            encoders,
            #[cfg(target_os = "macos")]
            instance_buttons: match (desc.product_id.0, target) {
                (0x1600, Target::Left) => [0x40, 0x42, 0x43],
                (0x1600, Target::Right) => [0x44, 0x46, 0x47],
                (_, Target::Left) => [4, 6, 7],
                (_, Target::Right) => [0, 2, 3],
            },
            #[cfg(target_os = "macos")]
            instance_encoders: match target {
                Target::Left => [0, 2, 3],
                Target::Right => [4, 6, 7],
            },
        })
    }

    #[cfg(target_os = "macos")]
    fn instance_event(self, event: InstanceInput) -> Option<Control> {
        match event {
            InstanceInput::Button { id, pressed } => {
                let index = self
                    .instance_buttons
                    .iter()
                    .position(|&button| button == id)?;
                Some(Control::Key(index, pressed))
            }
            InstanceInput::Encoder { id, delta } => {
                let index = self
                    .instance_encoders
                    .iter()
                    .position(|&encoder| encoder == id)?;
                let ticks = (delta * 1000.0).round().clamp(-127.0, 127.0) as i32;
                (ticks != 0).then_some(Control::Encoder(index, ticks))
            }
        }
    }

    fn event(self, event: Event) -> Option<Control> {
        match event {
            Event::Button { name, pressed, .. } => {
                let index = self.buttons.iter().position(|button| *button == name)?;
                Some(Control::Key(index, pressed))
            }
            Event::Encoder { name, delta, .. } if delta != 0 => {
                let index = self.encoders.iter().position(|encoder| *encoder == name)?;
                Some(Control::Encoder(index, delta))
            }
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Control {
    Key(usize, bool),
    Encoder(usize, i32),
}

pub type SharedKeys = Arc<Mutex<[usize; 3]>>;

pub struct Controls {
    map: ControlMap,
    socket: UdpSocket,
    port: u16,
    held: [bool; 3],
    shared_keys: SharedKeys,
    encoder_remainder: [i64; 3],
    midi_release: [Option<Instant>; 3],
    #[cfg(target_os = "macos")]
    midi: Option<MidiButtons>,
    #[cfg(target_os = "macos")]
    recent_hid: [Option<Instant>; 3],
}

impl Controls {
    pub fn new(
        map: ControlMap,
        descriptor: &DeviceDescriptor,
        target: Target,
        midi_fallback: bool,
        shared_keys: SharedKeys,
    ) -> io::Result<Self> {
        let mut controls = Self::bind(map, VERDANDI_OSC_PORT, shared_keys)?;
        #[cfg(target_os = "macos")]
        if midi_fallback {
            match MidiButtons::open(descriptor, target) {
                Ok(midi) => controls.midi = Some(midi),
                Err(error) => tracing::debug!(%error, "MIDI button input unavailable"),
            }
        }
        Ok(controls)
    }

    fn bind(map: ControlMap, port: u16, shared_keys: SharedKeys) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            map,
            socket,
            port,
            held: [false; 3],
            shared_keys,
            encoder_remainder: [0; 3],
            midi_release: [None; 3],
            #[cfg(target_os = "macos")]
            midi: None,
            #[cfg(target_os = "macos")]
            recent_hid: [None; 3],
        })
    }

    pub fn event(&mut self, event: Event) {
        if let Event::Button { name, pressed, .. } = &event {
            tracing::debug!(name, pressed, "Received controller button");
        }
        if let Some(control) = self.map.event(event) {
            #[cfg(target_os = "macos")]
            if let Control::Key(index, _) = control {
                self.recent_hid[index] = Some(Instant::now());
                self.midi_release[index] = None;
            }
            self.forward(control);
        }
    }

    #[cfg(target_os = "macos")]
    pub fn instance_event(&mut self, event: InstanceInput) -> Option<usize> {
        tracing::trace!(?event, "Received NI instance input");
        let control = self.map.instance_event(event)?;
        let pressed = match control {
            Control::Key(index, true) if !self.held[index] => Some(index),
            _ => None,
        };
        self.forward(control);
        pressed
    }

    pub fn poll_midi(&mut self, active: bool) {
        #[cfg(target_os = "macos")]
        {
            let now = Instant::now();
            for index in 0..self.midi_release.len() {
                if self.midi_release[index].is_some_and(|release| now >= release) {
                    self.midi_release[index] = None;
                    self.forward(Control::Key(index, false));
                }
            }
            let Some(taps) = self.midi.as_ref().map(|midi| midi.taps.clone()) else {
                return;
            };
            for index in taps.try_iter().take(32) {
                let recently_reported_by_hid = self.recent_hid[index]
                    .is_some_and(|last| now.duration_since(last) < Duration::from_millis(100));
                let held_by_hid = self.held[index] && self.midi_release[index].is_none();
                if !active || recently_reported_by_hid || held_by_hid {
                    continue;
                }
                if self.midi_release[index].take().is_some() {
                    self.forward(Control::Key(index, false));
                }
                self.forward(Control::Key(index, true));
                self.midi_release[index] = Some(now + TAP_DURATION);
            }
        }
    }

    pub fn release_all(&mut self) {
        self.midi_release.fill(None);
        self.encoder_remainder.fill(0);
        for index in 0..self.held.len() {
            if self.held[index] {
                self.forward(Control::Key(index, false));
            }
        }
    }

    fn forward(&mut self, control: Control) {
        let control = if let Control::Encoder(index, ticks) = control {
            let ticks_per_step = i64::from(ENCODER_TICKS_PER_STEP);
            let total = i64::from(ticks) + self.encoder_remainder[index];
            self.encoder_remainder[index] = total % ticks_per_step;
            let steps = (total / ticks_per_step) as i32;
            if steps == 0 {
                return;
            }
            Control::Encoder(index, steps)
        } else {
            control
        };
        let Control::Key(index, pressed) = control else {
            self.send(&control);
            return;
        };
        if self.held[index] == pressed {
            return;
        }
        let mut keys = self.shared_keys.lock().unwrap();
        let count = &mut keys[index];
        let next = if pressed { *count + 1 } else { *count - 1 };
        // Verdandi sees a single held key until every controller releases it.
        if (*count == 0 || next == 0) && !self.send(&control) && pressed {
            return;
        }
        self.held[index] = pressed;
        *count = next;
    }

    fn send(&self, control: &Control) -> bool {
        match self
            .socket
            .send_to(&osc_packet(control), (Ipv4Addr::LOCALHOST, self.port))
        {
            Ok(_) => {
                tracing::debug!(?control, "Sent controller input to Verdandi");
                true
            }
            Err(error) => {
                tracing::debug!(%error, ?control, "Could not send controller input");
                false
            }
        }
    }
}

impl Drop for Controls {
    fn drop(&mut self) {
        self.release_all();
    }
}

fn osc_packet(control: &Control) -> [u8; 24] {
    let (address, index, value) = match *control {
        Control::Key(index, pressed) => (b"/remote/key\0", index, i32::from(pressed)),
        Control::Encoder(index, delta) => (b"/remote/enc\0", index, delta),
    };
    let mut packet = [0; 24];
    packet[..12].copy_from_slice(address);
    packet[12..16].copy_from_slice(b",ii\0");
    packet[16..20].copy_from_slice(&(index as i32 + 1).to_be_bytes());
    packet[20..24].copy_from_slice(&value.to_be_bytes());
    packet
}

#[cfg(test)]
mod tests {
    use super::*;
    use encdr::{
        device::{loader::DescriptorRegistry, parser::PacketParser},
        DeviceId,
    };
    use std::time::Duration;

    #[cfg(target_os = "macos")]
    #[test]
    fn routes_norns_named_registration_and_native_s49_notifications() {
        use encdr::usb::ni_service::protocol::{instance_setup_named, parse_instance_input};

        let setup = instance_setup_named(0x1610, c"NORNS");
        assert_eq!(&setup[3][12..], b"\x06\0\0\0NORNS\0");

        let mut registry = DescriptorRegistry::new();
        registry.load_builtins().unwrap();
        let descriptor = registry.find(0x17cc, 0x1610).unwrap();
        let left = ControlMap::for_device(descriptor, Target::Left).unwrap();
        let right = ControlMap::for_device(descriptor, Target::Right).unwrap();
        let packet = |kind: u32, id, value| {
            [kind, 0xb5, 0, 1, id, value]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>()
        };

        for (position, button, index) in [(0, 4, 0), (2, 6, 1), (3, 7, 2)] {
            for pressed in [true, false] {
                let message = packet(0x03734e00, button, u32::from(pressed));
                let event = parse_instance_input(&message).unwrap();
                assert_eq!(
                    left.instance_event(event),
                    Some(Control::Key(index, pressed))
                );
                assert_eq!(right.instance_event(event), None);
            }
            let message = packet(0x03654e00, position, (-0.009f32).to_bits());
            let event = parse_instance_input(&message).unwrap();
            assert_eq!(
                left.instance_event(event),
                Some(Control::Encoder(index, -9))
            );
            assert_eq!(right.instance_event(event), None);
        }
        let knob_touch = parse_instance_input(&packet(0x03734e00, 0x37, 1)).unwrap();
        assert_eq!(left.instance_event(knob_touch), None);
        assert!(parse_instance_input(&packet(0x03654e00, 0, f32::NAN.to_bits())).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn routes_maschine_buttons_with_trailing_bytes_and_ignores_knob_touch() {
        use encdr::usb::ni_service::protocol::parse_instance_input;

        let mut registry = DescriptorRegistry::new();
        registry.load_builtins().unwrap();
        let descriptor = registry.find(0x17cc, 0x1600).unwrap();
        let left = ControlMap::for_device(descriptor, Target::Left).unwrap();
        let right = ControlMap::for_device(descriptor, Target::Right).unwrap();
        for (index, id) in [0x40u32, 0x42, 0x43].into_iter().enumerate() {
            for pressed in [true, false] {
                let packet: Vec<_> = [
                    0x0373_4e00,
                    0x25f,
                    0,
                    1,
                    id,
                    0x6d84_0d00 | u32::from(pressed),
                ]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect();
                let event = parse_instance_input(&packet).unwrap();
                assert_eq!(
                    left.instance_event(event),
                    Some(Control::Key(index, pressed))
                );
                assert_eq!(right.instance_event(event), None);
            }
        }
        assert_eq!(
            left.instance_event(InstanceInput::Button {
                id: 0x4f,
                pressed: true
            }),
            None
        );
        assert_eq!(
            left.instance_event(InstanceInput::Encoder {
                id: 2,
                delta: 0.025
            }),
            Some(Control::Encoder(1, 25))
        );
    }

    #[test]
    fn s49_hid_packets_map_the_bank_on_the_selected_side() {
        let mut registry = DescriptorRegistry::new();
        registry.load_builtins().unwrap();
        let descriptor = registry.find(0x17cc, 0x1610).unwrap().clone();
        let left = ControlMap::for_device(&descriptor, Target::Left).unwrap();
        let right = ControlMap::for_device(&descriptor, Target::Right).unwrap();
        let names = registry.intern_descriptor_names(&descriptor);
        let mut parser = PacketParser::new(DeviceId::default(), &descriptor, names);
        let mut buttons = [0; 32];
        buttons[0] = 0x01;
        buttons[7] = 0xf0; // Capacitive knob touch is not a display switch.
        let mut events = vec![];
        parser.parse_from("control", &buttons, &mut events);
        assert!(!events
            .iter()
            .cloned()
            .any(|e| left.event(e.clone()).is_some() || right.event(e).is_some()));
        for (left_mask, right_mask, index) in [
            (0x10, 0x01, Some(0)),
            (0x20, 0x02, None),
            (0x40, 0x04, Some(1)),
            (0x80, 0x08, Some(2)),
        ] {
            buttons[1] = left_mask | right_mask;
            events.clear();
            parser.parse_from("control", &buttons, &mut events);
            for bank in [left, right] {
                assert_eq!(
                    events
                        .iter()
                        .cloned()
                        .filter_map(|e| bank.event(e))
                        .collect::<Vec<_>>(),
                    index
                        .map(|i| Control::Key(i, true))
                        .into_iter()
                        .collect::<Vec<_>>()
                );
            }
            buttons[1] = 0;
            events.clear();
            parser.parse_from("control", &buttons, &mut events);
            for bank in [left, right] {
                assert_eq!(
                    events
                        .iter()
                        .cloned()
                        .filter_map(|e| bank.event(e))
                        .collect::<Vec<_>>(),
                    index
                        .map(|i| Control::Key(i, false))
                        .into_iter()
                        .collect::<Vec<_>>()
                );
            }
        }

        let mut knobs = [0; 51];
        events.clear();
        parser.parse_from("control", &knobs, &mut events);
        events.clear();
        knobs[17] = 1; // left position 1
        knobs[19] = 2; // left position 2, skipped
        knobs[21] = 3; // left position 3
        knobs[23] = 4; // left position 4
        knobs[25] = 15; // right position 1
        knobs[27] = 14; // right position 2, skipped
        knobs[29] = 13; // right position 3
        knobs[31] = 12; // right position 4
        parser.parse_from("control", &knobs, &mut events);
        for (bank, direction) in [(left, 1), (right, -1)] {
            assert_eq!(
                events
                    .iter()
                    .cloned()
                    .filter_map(|e| bank.event(e))
                    .collect::<Vec<_>>(),
                [
                    Control::Encoder(0, direction),
                    Control::Encoder(1, 3 * direction),
                    Control::Encoder(2, 4 * direction)
                ]
            );
        }
    }

    #[test]
    fn accumulates_encoder_ticks_per_knob_and_clears_them_on_handoff() {
        let map = ControlMap {
            buttons: ["top_1", "top_3", "top_4"],
            encoders: ["encoder_1", "encoder_3", "encoder_4"],
            #[cfg(target_os = "macos")]
            instance_buttons: [4, 6, 7],
            #[cfg(target_os = "macos")]
            instance_encoders: [0, 2, 3],
        };
        let listener = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut controls = Controls::bind(
            map,
            listener.local_addr().unwrap().port(),
            SharedKeys::default(),
        )
        .unwrap();

        let partial = ENCODER_TICKS_PER_STEP / 2;
        let rest = ENCODER_TICKS_PER_STEP - partial;
        controls.forward(Control::Encoder(0, partial));
        controls.forward(Control::Encoder(1, -partial));
        controls.forward(Control::Encoder(0, rest));
        controls.release_all();
        controls.forward(Control::Encoder(1, -partial));
        controls.forward(Control::Encoder(1, -rest));
        if partial != 0 {
            controls.forward(Control::Encoder(1, -rest));
        }

        let mut packet = [0; 24];
        for (index, step) in [(0, 1), (1, -1)] {
            assert_eq!(listener.recv(&mut packet).unwrap(), packet.len());
            assert_eq!(packet, osc_packet(&Control::Encoder(index, step)));
        }
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.recv(&mut packet).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn holds_a_shared_key_until_the_last_controller_releases_it() {
        let mut registry = DescriptorRegistry::new();
        registry.load_builtins().unwrap();
        let listener = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let keys = SharedKeys::default();
        let mut s49 = Controls::bind(
            ControlMap::for_device(registry.find(0x17cc, 0x1610).unwrap(), Target::Left).unwrap(),
            port,
            keys.clone(),
        )
        .unwrap();
        let mut maschine = Controls::bind(
            ControlMap::for_device(registry.find(0x17cc, 0x1600).unwrap(), Target::Left).unwrap(),
            port,
            keys,
        )
        .unwrap();

        s49.forward(Control::Key(0, true));
        maschine.forward(Control::Key(0, true));
        drop(s49);
        maschine.forward(Control::Encoder(2, ENCODER_TICKS_PER_STEP));
        maschine.release_all();

        let mut packet = [0; 24];
        for event in [
            Control::Key(0, true),
            Control::Encoder(2, 1),
            Control::Key(0, false),
        ] {
            assert_eq!(listener.recv(&mut packet).unwrap(), packet.len());
            assert_eq!(packet, osc_packet(&event));
        }
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.recv(&mut packet).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn sends_osc_presses_releases_and_signed_encoder_deltas() {
        let map = ControlMap {
            buttons: ["top_1", "top_3", "top_4"],
            encoders: ["encoder_1", "encoder_3", "encoder_4"],
            #[cfg(target_os = "macos")]
            instance_buttons: [4, 6, 7],
            #[cfg(target_os = "macos")]
            instance_encoders: [0, 2, 3],
        };
        let listener = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut controls = Controls::bind(
            map,
            listener.local_addr().unwrap().port(),
            SharedKeys::default(),
        )
        .unwrap();
        controls.event(Event::Button {
            device: DeviceId::default(),
            name: "top_3",
            pressed: true,
        });
        controls.event(Event::Encoder {
            device: DeviceId::default(),
            name: "encoder_4",
            delta: -2 * ENCODER_TICKS_PER_STEP,
        });
        drop(controls);

        let mut packet = [0; 24];
        for expected in [
            b"/remote/key\0,ii\0\0\0\0\x02\0\0\0\x01",
            b"/remote/enc\0,ii\0\0\0\0\x03\xff\xff\xff\xfe",
            b"/remote/key\0,ii\0\0\0\0\x02\0\0\0\0",
        ] {
            assert_eq!(listener.recv(&mut packet).unwrap(), packet.len());
            assert_eq!(&packet, expected);
        }
    }
}
