#[cfg(target_os = "macos")]
use crate::button_lights::ButtonLights;
use crate::config::Target;
use crate::controls::{ControlMap, Controls, SharedKeys};
use crate::framebuffer::{render, CapturedFrame};
use crossbeam_channel::{Receiver, Sender};
use encdr::core::descriptor::ScreenDesc;
use encdr::device::loader::DescriptorRegistry;
use encdr::screen::{protocol, ScreenManager};
use encdr::usb::device_thread::{DeviceCmd, DeviceHandle};
use encdr::usb::hotplug::{self, DetectedDevice};
use encdr::usb::ni_service::{is_rejected_frame, ServiceSession};
use encdr::{DeviceDescriptor, DeviceId, Event, PixelFormat};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const INSTANCE_NAME: &str = "NORNS";
const SCAN_INTERVAL: Duration = Duration::from_secs(1);
const INSTANCE_PROBE_INTERVAL: Duration = Duration::from_secs(2);

pub struct EncdrOutput {
    registry: DescriptorRegistry,
    selected: Vec<Selected>,
    shared_keys: SharedKeys,
    device_status: HashMap<DeviceId, String>,
    target: Target,
    device: Option<String>,
    force_usb: bool,
    enable_controls: bool,
    next_scan: Instant,
    events: (Sender<Event>, Receiver<Event>),
    last_status: String,
}

struct Selected {
    id: DeviceId,
    label: String,
    screen: ScreenDesc,
    controls: Option<Controls>,
    backend: Backend,
    displayed: Option<Arc<CapturedFrame>>,
}

enum Backend {
    Service {
        session: ServiceSession,
        display: u8,
        manager: Box<ScreenManager>,
        ownership: InstanceOwnership,
        blank: Option<BlankDisplay>,
        #[cfg(target_os = "macos")]
        lights: Option<Box<ButtonLights>>,
    },
    Usb(DeviceHandle),
}

struct BlankDisplay {
    index: u8,
    width: u16,
    height: u16,
    commands: Vec<u8>,
}

impl BlankDisplay {
    fn new(descriptor: &DeviceDescriptor, active: &ScreenDesc) -> Option<Self> {
        let name = if active.name == "left" {
            "right"
        } else {
            "left"
        };
        let secondary = screen(descriptor, name)?;
        if secondary.interface != active.interface {
            return None;
        }
        let black = vec![secondary.pixel_format.black_fill(); secondary.byte_size()];
        let blit = protocol::build_full_blit(secondary, &black);
        Some(Self {
            index: service_index(descriptor, secondary)?,
            width: secondary.width,
            height: secondary.height,
            commands: protocol::frame_transfers(secondary, blit).concat(),
        })
    }

    fn send(&self, session: &ServiceSession) -> Result<(), String> {
        session.send_display(self.index, self.width, self.height, &self.commands)
    }
}

#[derive(Clone, Copy, Debug, Default)]
enum InstanceOwnership {
    #[default]
    AwaitingFrame,
    Active {
        confirmed_at: Instant,
    },
    Paused {
        retry_at: Instant,
    },
}

impl InstanceOwnership {
    fn active(self) -> bool {
        matches!(self, Self::Active { .. })
    }

    fn notify(&mut self, active: bool, now: Instant) -> bool {
        match (active, *self) {
            (true, Self::Paused { .. }) => *self = Self::AwaitingFrame,
            (false, Self::Active { .. } | Self::AwaitingFrame) => self.pause(now),
            _ => return false,
        }
        true
    }

    fn pause(&mut self, now: Instant) {
        *self = Self::Paused {
            retry_at: now + INSTANCE_PROBE_INTERVAL,
        };
    }

    fn may_submit(self, now: Instant, changed: bool) -> bool {
        match self {
            Self::AwaitingFrame => true,
            Self::Active { confirmed_at } => {
                changed || now.duration_since(confirmed_at) >= INSTANCE_PROBE_INTERVAL
            }
            Self::Paused { retry_at } => now >= retry_at,
        }
    }

    fn confirm(&mut self, now: Instant) {
        *self = Self::Active { confirmed_at: now };
    }
}

impl EncdrOutput {
    pub fn new(
        target: Target,
        device: Option<String>,
        force_usb: bool,
        enable_controls: bool,
    ) -> Result<Self, String> {
        let mut registry = DescriptorRegistry::new();
        registry
            .load_builtins()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            registry,
            selected: Vec::new(),
            shared_keys: SharedKeys::default(),
            device_status: HashMap::new(),
            target,
            device,
            force_usb,
            enable_controls,
            next_scan: Instant::now(),
            events: crossbeam_channel::unbounded(),
            last_status: String::new(),
        })
    }

    pub fn list(&self) {
        let mut devices: Vec<_> = hotplug::scan_devices(&self.registry)
            .into_iter()
            .filter(|device| dual_screens(&device.descriptor).is_some())
            .map(|device| device_label(&device))
            .collect();
        devices.sort();
        if devices.is_empty() {
            println!("No Encdr-compatible dual-screen NI controller found.");
        }
        for device in devices {
            println!("{device}");
        }
    }

    fn scan(&mut self) {
        self.next_scan = Instant::now() + SCAN_INTERVAL;
        let scanned = hotplug::scan_devices(&self.registry);
        self.selected.retain(|selected| {
            let connected = scanned.iter().any(|device| device.device_id == selected.id);
            if !connected {
                tracing::info!(device = %selected.label, "NI controller disconnected");
            }
            connected
        });
        self.device_status
            .retain(|id, _| scanned.iter().any(|device| device.device_id == *id));

        let matches: Vec<_> = scanned
            .iter()
            .filter(|device| {
                dual_screens(&device.descriptor).is_some()
                    && matches_device(
                        &device.descriptor.name,
                        device.usb_info.serial_number(),
                        self.device.as_deref(),
                    )
            })
            .collect();
        if matches.is_empty() {
            let model = self
                .device
                .as_deref()
                .unwrap_or("dual-screen NI controller");
            self.status(format!(
                "Waiting for supported NI device matching '{model}'."
            ));
            return;
        }
        // Standalone USB keeps its explicit single-controller selection.
        if self.force_usb && matches.len() > 1 && self.selected.is_empty() {
            let mut names: Vec<_> = matches.iter().map(|device| device_label(device)).collect();
            names.sort();
            self.status(format!(
                "Multiple standalone USB controllers match. Use --device to select one: {}",
                names.join(", ")
            ));
            return;
        }
        self.last_status.clear();

        for device in matches {
            if self
                .selected
                .iter()
                .any(|selected| selected.id == device.device_id)
                || (self.force_usb && !self.selected.is_empty())
            {
                continue;
            }
            if !self.force_usb
                && scanned
                    .iter()
                    .filter(|other| {
                        other.descriptor.product_id == device.descriptor.product_id
                            && other.descriptor.vendor_id == device.descriptor.vendor_id
                    })
                    .count()
                    > 1
            {
                // Encdr may fall back to the first serial for this model.
                self.device_status(
                    device.device_id,
                    format!(
                        "Connect one unit of this model to use NI instance switching: {}",
                        device_label(device)
                    ),
                );
                continue;
            }
            self.connect(device);
        }
        self.next_scan = Instant::now() + SCAN_INTERVAL;
    }

    fn connect(&mut self, device: &DetectedDevice) {
        let label = device_label(device);
        let screen = screen(&device.descriptor, self.target.name())
            .expect("device capabilities checked during discovery")
            .clone();
        let backend = if self.force_usb {
            self.open_usb(device).map(Backend::Usb)
        } else if let Some(quirk) = device
            .descriptor
            .quirks
            .ni_service
            .as_ref()
            .filter(|quirk| quirk.screen_interface == screen.interface)
        {
            let display = service_index(&device.descriptor, &screen)
                .expect("display was obtained from this descriptor");
            #[cfg(target_os = "macos")]
            let session = ServiceSession::connect_named(
                quirk.product_id.0,
                device.usb_info.serial_number(),
                INSTANCE_NAME,
                Duration::from_secs(2),
            );
            #[cfg(not(target_os = "macos"))]
            let session = ServiceSession::connect(
                quirk.product_id.0,
                device.usb_info.serial_number(),
                Duration::from_secs(2),
            );
            session.map(|session| Backend::Service {
                session,
                display,
                manager: Box::new(ScreenManager::new(&screen, None)),
                ownership: InstanceOwnership::default(),
                blank: BlankDisplay::new(&device.descriptor, &screen),
                #[cfg(target_os = "macos")]
                lights: self
                    .enable_controls
                    .then(|| ButtonLights::new(&device.descriptor, self.target))
                    .flatten()
                    .map(Box::new),
            })
        } else {
            self.device_status(device.device_id, format!("{label} has no NI instance support in Encdr. Use --usb only for standalone USB operation."));
            return;
        };

        match backend {
            Ok(backend) => {
                let transport = match &backend {
                    Backend::Service { .. } => "NI instance NORNS",
                    Backend::Usb(_) => "standalone USB",
                };
                self.device_status(
                    device.device_id,
                    format!(
                        "Selected {label} · {} {}×{} · Encdr {transport}",
                        screen.name, screen.width, screen.height
                    ),
                );
                let controls = self
                    .enable_controls
                    .then(|| ControlMap::for_device(&device.descriptor, self.target))
                    .flatten()
                    .and_then(|map| {
                        match Controls::new(
                            map,
                            &device.descriptor,
                            self.target,
                            matches!(&backend, Backend::Usb(_)),
                            self.shared_keys.clone(),
                        ) {
                            Ok(controls) => Some(controls),
                            Err(error) => {
                                tracing::warn!(%error, "Controller input unavailable");
                                None
                            }
                        }
                    });
                if controls.is_some() {
                    tracing::info!(
                        device = %label,
                        bank = self.target.name(),
                        "Mapped display button and knob positions 1, 3 and 4 to Verdandi"
                    );
                }

                self.selected.push(Selected {
                    id: device.device_id,
                    label,
                    screen,
                    backend,
                    displayed: None,
                    controls,
                });
            }
            Err(error) => {
                self.device_status(device.device_id, format!("Waiting for {label}: {error}"))
            }
        }
    }

    fn open_usb(&mut self, device: &DetectedDevice) -> Result<DeviceHandle, String> {
        // Encdr initializes both standalone USB screens to black; only the
        // selected display receives mirror frames after initialization.
        let mut descriptor = (*device.descriptor).clone();
        descriptor.quirks.ni_service = None;
        let names = self.registry.intern_descriptor_names(&descriptor);
        DeviceHandle::spawn(
            device.device_id,
            Arc::new(descriptor),
            &device.usb_info,
            names,
            self.events.0.clone(),
            None,
        )
        .map_err(|error| error.to_string())
    }

    pub fn update(&mut self, latest: &Mutex<Option<Arc<CapturedFrame>>>, submit_frame: bool) {
        let latest = latest.lock().unwrap().clone();
        if Instant::now() >= self.next_scan && (!self.selected.is_empty() || latest.is_some()) {
            self.scan();
        }
        self.selected.retain_mut(|selected| {
            match selected.update(latest.clone(), submit_frame, &self.events.1) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(device = %selected.label, %error, "NI connection lost; reconnecting");
                    self.device_status.remove(&selected.id);
                    false
                }
            }
        });
        if self.force_usb && self.selected.is_empty() {
            for _ in self.events.1.try_iter().take(256) {}
        }
    }

    fn device_status(&mut self, id: DeviceId, message: String) {
        if self.device_status.get(&id) != Some(&message) {
            eprintln!("{message}");
            self.device_status.insert(id, message);
        }
    }

    fn status(&mut self, message: String) {
        if self.last_status != message {
            eprintln!("{message}");
            self.last_status = message;
        }
    }
}

impl Selected {
    fn update(
        &mut self,
        latest: Option<Arc<CapturedFrame>>,
        submit_frame: bool,
        usb_events: &Receiver<Event>,
    ) -> Result<(), String> {
        let span = tracing::debug_span!("controller", device = %self.label);
        let _entered = span.enter();
        if matches!(&self.backend, Backend::Usb(handle) if !handle.is_connected()) {
            return Err("NI USB connection closed".into());
        }
        let was_active = self.has_instance_control();
        let mut changed = self.poll_instance()?;
        let has_source = latest.is_some();
        if let (true, Some(frame)) = (submit_frame, latest) {
            self.submit(frame)?;
        }
        changed |= self.poll_instance()?;
        // Drop input queued across this controller's handoff.
        let owns_source = has_source && self.has_instance_control();
        let controls_active = owns_source && was_active && !changed;
        if !controls_active {
            if let Some(controls) = &mut self.controls {
                controls.release_all();
            }
        }
        match &mut self.backend {
            Backend::Usb(_) => {
                for event in usb_events.try_iter().take(256) {
                    if controls_active && event.device_id() == self.id {
                        if let Some(controls) = &mut self.controls {
                            controls.event(event);
                        }
                    }
                }
                if let Some(controls) = &mut self.controls {
                    controls.poll_midi(controls_active);
                }
            }
            Backend::Service {
                session,
                #[cfg(target_os = "macos")]
                lights,
                ..
            } => {
                #[cfg(target_os = "macos")]
                {
                    if !controls_active {
                        if let Some(lights) = lights.as_mut() {
                            lights.reset();
                        }
                    }
                    let now = Instant::now();
                    let events = session.input_events();
                    for event in std::iter::from_fn(|| events.try_recv().ok()).take(256) {
                        if controls_active {
                            if let Some(controls) = &mut self.controls {
                                if let (Some(index), Some(lights)) =
                                    (controls.instance_event(event), lights.as_mut())
                                {
                                    lights.press(index, now);
                                }
                            }
                        }
                    }
                    if owns_source {
                        if let Some(lights) = lights.as_mut() {
                            lights.update(session, now);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn has_instance_control(&self) -> bool {
        match &self.backend {
            Backend::Usb(handle) => handle.has_control(),
            Backend::Service { ownership, .. } => ownership.active(),
        }
    }

    fn poll_instance(&mut self) -> Result<bool, String> {
        let Backend::Service {
            session,
            manager,
            ownership,
            ..
        } = &mut self.backend
        else {
            return Ok(false);
        };
        let control = session.control_events();
        if control.is_closed() {
            return Err("NI instance notifications closed".into());
        }
        let mut changed = false;
        while let Ok(active) = control.try_recv() {
            if ownership.notify(active, Instant::now()) {
                changed = true;
                if !active {
                    tracing::info!(
                        device = %self.label,
                        "Another NI instance is active; Verdandi has yielded the controller"
                    );
                }
            }
        }
        if changed {
            self.displayed = None;
            **manager = ScreenManager::new(&self.screen, None);
        }
        Ok(changed)
    }

    fn submit(&mut self, frame: Arc<CapturedFrame>) -> Result<(), String> {
        match &mut self.backend {
            Backend::Service {
                session,
                display: display_index,
                manager,
                ownership,
                blank,
                ..
            } => {
                let changed = self.displayed.as_deref() != Some(frame.as_ref());
                if !ownership.may_submit(Instant::now(), changed) {
                    return Ok(());
                }
                let blit = if changed {
                    let rgba = render(&frame, self.screen.width, self.screen.height)
                        .ok_or("invalid display frame")?;
                    match manager.submit(&rgba, PixelFormat::Rgba8888, &self.screen) {
                        Some(blit) => blit,
                        None => {
                            self.displayed = Some(frame);
                            return Ok(());
                        }
                    }
                } else {
                    // When Verdandi is idle, revalidate the session using its
                    // existing frame so a lost service cannot leave input enabled.
                    manager.current_blit(&self.screen)
                };
                let commands = protocol::frame_transfers(&self.screen, blit);
                let result = if let [single] = commands.as_slice() {
                    session.send_display(
                        *display_index,
                        self.screen.width,
                        self.screen.height,
                        single,
                    )
                } else {
                    session.send_display(
                        *display_index,
                        self.screen.width,
                        self.screen.height,
                        &commands.concat(),
                    )
                };
                let result = result.and_then(|()| {
                    if !ownership.active() {
                        if let Some(blank) = blank {
                            blank.send(session)?;
                            tracing::info!(device = %self.label, display_index = blank.index, "Cleared the other display for NORNS");
                        }
                    }
                    Ok(())
                });
                if let Err(error) = result {
                    if !is_rejected_frame(&error) {
                        return Err(error);
                    }
                    if !matches!(ownership, InstanceOwnership::Paused { .. }) {
                        tracing::info!(device = %self.label, "NI instance is inactive. Select NORNS on the controller.");
                    }
                    ownership.pause(Instant::now());
                    **manager = ScreenManager::new(&self.screen, None);
                    self.displayed = None;
                    return Ok(());
                }
                if !ownership.active() {
                    tracing::info!(device = %self.label, "Verdandi’s NI instance has control");
                }
                ownership.confirm(Instant::now());
                tracing::debug!(device = %self.label, screen_index = *display_index, "NI instance accepted frame");
            }
            Backend::Usb(handle) => {
                if !handle.has_control() {
                    self.displayed = None;
                    return Ok(());
                }
                if self.displayed.as_deref() == Some(frame.as_ref()) || handle.screen_backlog() > 0
                {
                    return Ok(());
                }
                let pixels = render(&frame, self.screen.width, self.screen.height)
                    .ok_or("invalid display frame")?;
                handle.send(DeviceCmd::SubmitScreen {
                    screen: self.screen.name.clone(),
                    pixels,
                    format: PixelFormat::Rgba8888,
                });
                tracing::debug!(screen = self.screen.name, "Queued frame for Encdr USB");
            }
        }
        self.displayed = Some(frame);
        Ok(())
    }
}

fn screen<'a>(descriptor: &'a DeviceDescriptor, name: &str) -> Option<&'a ScreenDesc> {
    descriptor
        .screens
        .iter()
        .find(|screen| screen.name == name && screen.width > 0 && screen.height > 0)
}

fn dual_screens(descriptor: &DeviceDescriptor) -> Option<(&ScreenDesc, &ScreenDesc)> {
    Some((screen(descriptor, "left")?, screen(descriptor, "right")?))
}

fn service_index(descriptor: &DeviceDescriptor, screen: &ScreenDesc) -> Option<u8> {
    descriptor
        .screens
        .iter()
        .filter(|other| other.interface == screen.interface)
        .position(|other| other.name == screen.name)
        .and_then(|index| u8::try_from(index).ok())
}

fn matches_device(name: &str, serial: Option<&str>, selector: Option<&str>) -> bool {
    selector.is_none_or(|selector| {
        serial.is_some_and(|serial| serial.eq_ignore_ascii_case(selector))
            || name.to_lowercase().contains(&selector.to_lowercase())
    })
}

fn device_label(device: &DetectedDevice) -> String {
    let name = &device.descriptor.name;
    device
        .usb_info
        .serial_number()
        .map_or_else(|| name.clone(), |serial| format!("{name} [{serial}]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_payload_targets_the_other_service_display() {
        let mut registry = DescriptorRegistry::new();
        registry.load_builtins().unwrap();
        let descriptor = registry.find(0x17cc, 0x1610).unwrap();
        let (left, right) = dual_screens(descriptor).unwrap();
        for (active, other, expected_index) in [(left, right, 1), (right, left, 0)] {
            let blank = BlankDisplay::new(descriptor, active).unwrap();
            assert_eq!(blank.index, expected_index);
            assert_eq!((blank.width, blank.height), (480, 272));
            let payload = &blank.commands[blank.commands.len()
                - other.full_blit.footer.split(',').count()
                - other.byte_size()..];
            assert!(payload[..other.byte_size()].iter().all(|byte| *byte == 0));
        }
    }

    #[test]
    fn instance_requires_display_ack_and_yields_immediately_on_handoff() {
        let now = Instant::now();
        let mut ownership = InstanceOwnership::default();
        assert!(!ownership.active());
        assert!(ownership.may_submit(now, true));

        ownership.confirm(now);
        assert!(ownership.active());
        assert!(!ownership.may_submit(now, false));
        assert!(ownership.may_submit(now + INSTANCE_PROBE_INTERVAL, false));

        assert!(ownership.notify(false, now));
        assert!(!ownership.active());
        assert!(!ownership.may_submit(now, true));
        assert!(ownership.may_submit(now + INSTANCE_PROBE_INTERVAL, true));
        assert!(ownership.notify(true, now));
        assert!(!ownership.active());
        assert!(ownership.may_submit(now, true));

        ownership.confirm(now);
        assert!(ownership.active());
        assert!(!ownership.notify(true, now));
    }

    #[test]
    fn uses_descriptor_dimensions_and_preserves_service_display_numbers() {
        let mut registry = DescriptorRegistry::new();
        registry.load_builtins().unwrap();
        let s49 = registry.find(0x17cc, 0x1610).unwrap();
        let (left, right) = dual_screens(s49).unwrap();
        assert_eq!((left.width, left.height), (480, 272));
        assert_eq!(service_index(s49, left), Some(0));
        assert_eq!(service_index(s49, right), Some(1));
        let mk2 = registry.find(0x17cc, 0x1140).unwrap();
        assert_eq!(dual_screens(mk2).unwrap().0.width, 256);
        assert!(dual_screens(registry.find(0x17cc, 0x2200).unwrap()).is_none());
    }

    #[test]
    fn matches_model_and_serial() {
        assert!(matches_device(
            "NI S49 Mk2",
            Some("abc123"),
            Some("s49 MK2")
        ));
        assert!(matches_device("NI S49 Mk2", Some("abc123"), Some("ABC123")));
        assert!(!matches_device("NI S49 Mk2", Some("abc123"), Some("abc")));
        assert!(matches_device("NI S49 Mk2", None, None));
    }
}
