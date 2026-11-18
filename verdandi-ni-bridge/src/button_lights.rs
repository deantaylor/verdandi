use crate::config::Target;
use encdr::usb::ni_service::ServiceSession;
use encdr::DeviceDescriptor;
use std::time::{Duration, Instant};

const PULSE: Duration = Duration::from_millis(100);
const DIM_WHITE: u8 = 0x44;
const PULSE_WHITE: u8 = 0x45;

pub struct ButtonLights {
    levels: Vec<u8>,
    offsets: [usize; 3],
    until: [Option<Instant>; 3],
    sent: Option<[bool; 3]>,
    retry_at: Option<Instant>,
}

impl ButtonLights {
    pub fn new(device: &DeviceDescriptor, target: Target) -> Option<Self> {
        // NI service LED array lengths and display-button offsets, documented
        // in Rebellion's scripts/mappings.lua. These differ from USB reports.
        let (led_count, first) = match (device.vendor_id.0, device.product_id.0) {
            (0x17cc, 0x1600) => (103, 12),
            (0x17cc, 0x1610) => (130, 2),
            _ => return None,
        };
        let bank = first
            + if matches!(target, Target::Right) {
                4
            } else {
                0
            };
        Some(Self {
            levels: vec![0; led_count],
            offsets: [bank, bank + 2, bank + 3],
            until: [None; 3],
            sent: None,
            retry_at: None,
        })
    }

    pub fn reset(&mut self) {
        self.until.fill(None);
        self.sent = None;
        self.retry_at = None;
    }

    pub fn press(&mut self, index: usize, now: Instant) {
        self.until[index] = Some(now + PULSE);
    }

    pub fn update(&mut self, session: &ServiceSession, now: Instant) {
        if self.retry_at.is_some_and(|retry| now < retry) {
            return;
        }
        let pulsing = self.pulsing(now);
        if self.sent == Some(pulsing) {
            return;
        }
        for (offset, flash) in self.offsets.into_iter().zip(pulsing) {
            self.levels[offset] = if flash { PULSE_WHITE } else { DIM_WHITE };
        }
        match session.send_led_state(&self.levels) {
            Ok(()) => {
                self.sent = Some(pulsing);
                self.retry_at = None;
                tracing::debug!(?pulsing, "Updated NORNS button lights");
            }
            Err(error) => {
                tracing::debug!(%error, "Could not update NORNS button lights");
                self.retry_at = Some(now + Duration::from_secs(1));
            }
        }
    }

    fn pulsing(&self, now: Instant) -> [bool; 3] {
        self.until
            .map(|until| until.is_some_and(|until| now < until))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use encdr::device::loader::DescriptorRegistry;
    use encdr::usb::ni_service::protocol::led_state_message;

    #[test]
    fn uses_the_native_led_report_and_expires_only_the_pressed_button() {
        let mut registry = DescriptorRegistry::new();
        registry.load_builtins().unwrap();
        for (product, len, left) in [(0x1600, 103, [12, 14, 15]), (0x1610, 130, [2, 4, 5])] {
            let descriptor = registry.find(0x17cc, product).unwrap();
            for (side, bank) in [(Target::Left, 0), (Target::Right, 4)] {
                let mut lights = ButtonLights::new(descriptor, side).unwrap();
                assert_eq!(lights.offsets, left.map(|index| index + bank));
                let message = led_state_message(&lights.levels);
                assert_eq!(&message[..4], &[0x00, 0x75, 0x6c, 0x03]);
                assert_eq!(message.len(), len + 8);
                assert_eq!(
                    u32::from_le_bytes(message[4..8].try_into().unwrap()) as usize,
                    len
                );
                let now = Instant::now();
                lights.press(1, now);
                assert_eq!(lights.pulsing(now), [false, true, false]);
                assert_eq!(lights.pulsing(now + PULSE), [false; 3]);
                lights.reset();
                assert_eq!(lights.pulsing(now), [false; 3]);
            }
        }
    }
}
