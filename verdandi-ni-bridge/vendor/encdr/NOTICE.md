# Encdr 0.8.5 in the Verdandi NI bridge

This directory contains Encdr 0.8.5 by RufusIbiza and contributors under
GPL-3.0-or-later. See [LICENSE](LICENSE) and the [upstream source at commit
`4e1fa75aa329fc18823be88e6a0c1b385f53f6ac`](https://github.com/RufusIbiza/Encdr/tree/4e1fa75aa329fc18823be88e6a0c1b385f53f6ac).
The crate's [Cargo manifest](Cargo.toml) records the license identifier.

The Verdandi fork modified this copy on 2026-11-18. The changes are released
under GPL-3.0-or-later and affect three files in `src/usb/ni_service/`:

- `macos.rs` adds named instance registration, native input access and LED output.
- `mod.rs` routes native button and encoder notifications to the bridge.
- `protocol.rs` encodes the instance name and LED state and decodes native input.
