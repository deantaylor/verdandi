# Verdandi NI Bridge

Mirror Verdandi's live norns OLED onto a display on each compatible Native Instruments controller. The bridge reads completed OLED frames from Verdandi and sends them through Encdr. The left display is the default.

## Quick start

After building the app below, open **Verdandi Patched.app** from Finder.
It starts the bundled bridge and enables frame export automatically.
Quit the stock Verdandi session before opening Verdandi Patched.

The instance registers once Verdandi sends its first OLED frame and the NI
hardware service is available. On the S49 Mk2, press **INSTANCE** and select
**NORNS**.
See `~/Library/Logs/Verdandi Patched/bridge.log` for connection status. Closing Verdandi Patched stops its
bundled bridge.

### Run from Terminal with custom options

From the `verdandi-ni-bridge` directory, start the bridge with your preferred options:

```sh
./target/release/verdandi-ni-bridge --device "S49 Mk2"
```

In a second terminal, set the socket explicitly to use that bridge:

```sh
VERDANDI_FRAME_SOCKET="$(./target/release/verdandi-ni-bridge --print-socket)" \
  "../dist/ni-export/Verdandi Patched.app/Contents/MacOS/Verdandi"
```

For a custom socket or configuration file, pass the same option when calling
`--print-socket`. Ctrl-C stops the separately launched bridge.

## Build the bridge

From the root of the Verdandi fork, use Rust 1.90 or newer:

```sh
cd verdandi-ni-bridge
cargo build --release --locked
cargo test --locked
cd ..
```

## Build Verdandi Patched

The `ni-framebuffer-export` branch of the Verdandi fork already includes
the exporter and control feedback patch. After building the bridge, run
these commands from the fork root using
[Verdandi's macOS toolchain](../readme-macos.md):

```sh
git submodule update --init --recursive
macos/setup-deps.sh "$PWD/deps"
macos/build.sh
NORNS_APP_OUT="$PWD/dist/ni-export" macos/make-app.sh

app="$PWD/dist/ni-export/Verdandi Patched.app"
mv "$PWD/dist/ni-export/Verdandi.app" "$app"
/usr/libexec/PlistBuddy \
  -c 'Set :CFBundleName Verdandi Patched' \
  -c 'Set :CFBundleDisplayName Verdandi Patched' \
  -c 'Set :CFBundleIdentifier community.verdandi.app.niexport' \
  "$app/Contents/Info.plist"
verdandi-ni-bridge/scripts/bundle-bridge.sh "$app"
```

Open the resulting `dist/ni-export/Verdandi Patched.app` in Finder.
For manual launch, use the Terminal commands above. No Screen Recording permission is required.

### Reapply the patch after an upstream update

Keep the bridge build above and cherry-pick the exporter commit
`acb61ba6` onto the upstream version you want to patch:

```sh
version=vX.Y.Z
bridge="$PWD/verdandi-ni-bridge"
git fetch https://github.com/ha1dcre/verdandi.git tag "$version"
git worktree add -b "ni-$version" "../verdandi-ni-$version" FETCH_HEAD
cd "../verdandi-ni-$version"
git cherry-pick acb61ba6
```

Repeat the app build from the new worktree and use
`"$bridge/scripts/bundle-bridge.sh" "$app"` for the final command. Build a
separate patched copy whenever the stock updater installs a new version.

## Device and display selection

```sh
./target/release/verdandi-ni-bridge --list-devices
./target/release/verdandi-ni-bridge --device "S49 Mk2" --target left --fps 30
```

- Without `--device`, the bridge registers **NORNS** on each eligible NI service controller. Use `--device` with a case-insensitive model substring or exact USB serial to limit which controllers it uses.
- `--target left|right` selects the mirror display. In NI instance mode the bridge blanks the other display when NORNS gains ownership.
- `--fps 1..60` caps submissions. The default is 30. The real update rate depends on the source and transport.
- `--usb` is for standalone USB operation when the bridge is the only controller app. It bypasses NI instance switching. With instance switching and two units of the same model, connect just one unit so Encdr can register the correct one.
- `--socket PATH` overrides the socket. Provide the same absolute path to Verdandi. Use a short pathname in a directory owned by you with mode 0700. The default is under the macOS per-user temporary directory.

Optional `--config PATH` reads a `key=value` file. Command-line values take precedence.

```ini
device=S49 Mk2
target=left
fps=30
```

If the file sets `socket`, include `--config PATH` on the `--print-socket` command as well.

### Sharing a controller with another app

Keep Native Instruments' hardware service running and start the bridge
without `--usb`. Use the controller's Instance or Setup control to select
**NORNS**. Verdandi updates the selected display, blanks the other display,
and forwards mapped input notifications from the NI service once NORNS owns
that controller. Select **NORNS** on each controller you want to use. Both
can mirror the same Verdandi session and send input at the same time.
On handoff, the bridge releases that controller's keys and discards its queued
input. A key held on another controller stays down. The bridge sends its latest
OLED frame when control returns, even if Verdandi is idle.

If the NI service is unavailable, the bridge waits. If the NI agents are
installed but stopped, macOS can start them from Terminal:

```sh
ni_hardware="/Library/Application Support/Native Instruments/Hardware"
open -g "$ni_hardware/NIHardwareAgent.app"
open -g "$ni_hardware/NIHostIntegrationAgent.app"
```

Devices with raw USB only require `--usb` and cannot participate in
instance switching. Normal MIDI streams remain shared by macOS. Other apps
receiving MIDI directly can still act on it independently of NI instance
selection.

### Display controls

On Maschine Mk3 and Komplete Kontrol S49/S61/S88 Mk2, positions **1, 3 and 4** in the bank by
the selected display act as norns **K1–K3** for buttons and **E1–E3** for
knobs. To adjust sensitivity, change `ENCODER_TICKS_PER_STEP` in
[`src/controls.rs`](src/controls.rs) and rebuild the bridge. Higher values
make the knobs less sensitive. A quick K1 tap toggles the norns menu.
The patched app shows button presses and knob turns on its on-screen controls.
On the S49 Mk2 and Maschine Mk3, the three mapped buttons glow dim white
and brighten briefly on each press while NORNS is active.

NI instance mode uses the service’s native button and encoder notifications.
With standalone USB, the bridge also listens for factory MIDI button toggles
and turns them into short taps. Native button releases preserve K1 holds.

| Selected display | K1–K3 | E1–E3 |
| --- | --- | --- |
| Left (default) | Display buttons 1, 3, 4 | Display knobs 1, 3, 4 |
| Right | Display buttons 5, 7, 8 | Display knobs 5, 7, 8 |

The bridge forwards controls through Verdandi's existing local norns OSC
receiver at `127.0.0.1:10111`. This controls norns menus and scripts.
Use `--display-only` for mirroring by itself.

### Hardware

Eligibility comes from Encdr 0.8.5 descriptors with programmable displays named `left` and `right`. Screen dimensions, pixel format, framing and the service display index come from those descriptors.

| Encdr descriptors | Per display | Transport |
| --- | --- | --- |
| Komplete Kontrol S49/S61/S88 Mk2 | 480×272 color | NI service, optional raw USB |
| Maschine Mk3, Maschine Plus, Maschine Studio | 480×272 color | NI service, optional raw USB |
| Maschine Mk2 | 256×64 monochrome | Raw USB |
| Maschine Mk1 | 255×64 grayscale | Raw USB |
| Traktor Kontrol S5/S8 | 480×272 color | NI service, optional raw USB |
| Traktor Kontrol S4 Mk3 | 320×240 color | NI service, optional raw USB |

The S49 Mk2 and Maschine Mk3 have been tested together with NI instance switching, native controls, and button LEDs. The other models are descriptor-based candidates.

## Troubleshooting

- **Waiting for patched Verdandi.** Open Verdandi Patched in Finder or launch it from Terminal with `VERDANDI_FRAME_SOCKET` set. A normally launched stock app does not export frames. To inspect capture independently of the controller, stop the bridge and run it with `--snapshot /tmp/verdandi-oled.ppm` to save the next native frame.
- **Controller missing.** Run `--list-devices` from Terminal. If you use `--device`, check the model substring or serial against the list.
- **NI service unavailable or another instance is active.** Keep the Native Instruments hardware agent running and select NORNS on the controller. The bridge waits and reconnects automatically. For detailed logs, start it with `RUST_LOG=verdandi_ni_bridge=debug,encdr=info`.
- **Controller or Verdandi restarts.** Keep the bridge running. It resumes when frames and the device return. While the source is absent, it holds the last image already on the display.

## How it works

Verdandi keeps the completed OLED in an internal Cairo surface. The stock app's screenshot and GIF functions are disk exports, while Lua's `screen.peek()` samples the drawing surface at reduced precision. The opt-in exporter reads the finished RGB888 pixels before window scaling and sends a non-blocking local Unix datagram. Window size, Retina scaling, occlusion and skin geometry are outside the capture path.

The bridge validates each packet and keeps the newest image. It skips duplicate renders and periodically rechecks NI instance ownership. It centres an integer nearest-neighbour scale on black, with a proportional downscale for displays smaller than the source. The S49 Mk2 renders 128×64 at 3×, producing 384×192 at offset (48, 40) within 480×272. Encdr handles display conversion, partial updates, service communication and raw USB transport.

The local wire format is `VDF1`, little-endian 16-bit width and height, then tightly packed top-to-bottom RGB888. Version 1 accepts 128×64. Verdandi repeats the last frame when idle so a newly started or restarted bridge can attach.

## Third-party software and licensing

See [Third-party software and references](THIRD_PARTY.md) for the vendored Encdr
source and license, the dated list of local modifications, and the protocol
references used by the bridge.
