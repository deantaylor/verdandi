use crate::config::Target;
use crossbeam_channel::{bounded, Receiver};
use encdr::DeviceDescriptor;
use midir::{MidiInput, MidiInputConnection};

pub struct MidiButtons {
    _connection: MidiInputConnection<()>,
    pub taps: Receiver<usize>,
}

impl MidiButtons {
    pub fn open(descriptor: &DeviceDescriptor, target: Target) -> Result<Self, String> {
        let model = match descriptor.product_id.0 {
            0x1610 => "S49",
            0x1620 => "S61",
            0x1630 => "S88",
            _ => return Err("no MIDI button mapping for this model".into()),
        };
        let wanted = format!("KOMPLETE KONTROL {model} MK2 Port 1");
        let input = MidiInput::new("Verdandi NI Bridge").map_err(|error| error.to_string())?;
        let ports = input.ports();
        let mut matching = ports.iter().filter(|port| {
            input
                .port_name(port)
                .is_ok_and(|name| name.eq_ignore_ascii_case(&wanted))
        });
        let port = matching
            .next()
            .ok_or_else(|| format!("MIDI source '{wanted}' is unavailable"))?;
        if matching.next().is_some() {
            return Err(format!("more than one MIDI source named '{wanted}'"));
        }
        let (send, taps) = bounded(32);
        let connection = input
            .connect(
                port,
                "Display buttons",
                move |_, message, _| {
                    if let Some(index) = button_tap(message, target) {
                        let _ = send.try_send(index);
                    }
                },
                (),
            )
            .map_err(|error| error.to_string())?;
        tracing::info!(
            port = wanted,
            "Listening for display-button taps in MIDI mode"
        );
        Ok(Self {
            _connection: connection,
            taps,
        })
    }
}

fn button_tap(message: &[u8], target: Target) -> Option<usize> {
    // The factory MIDI template alternates CC values 0 and 127 on successive
    // taps. There is no separate message for the release.
    let [0xB0, cc, 0 | 127] = message else {
        return None;
    };
    let start = match target {
        Target::Left => 112,
        Target::Right => 116,
    };
    [start, start + 2, start + 3]
        .iter()
        .position(|button| button == cc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_both_toggle_values_and_skips_the_second_position() {
        for (target, start) in [(Target::Left, 112), (Target::Right, 116)] {
            for (index, offset) in [0, 2, 3].into_iter().enumerate() {
                for value in [0, 127] {
                    assert_eq!(
                        button_tap(&[0xb0, start + offset, value], target),
                        Some(index)
                    );
                }
            }
            assert_eq!(button_tap(&[0xb0, start + 1, 127], target), None);
            assert_eq!(button_tap(&[0xb0, 14, 127], target), None);
            assert_eq!(button_tap(&[0x90, start, 127], target), None);
            assert_eq!(button_tap(&[0xb0, start, 63], target), None);
        }
    }
}
