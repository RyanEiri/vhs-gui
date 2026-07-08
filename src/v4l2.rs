use std::collections::BTreeMap;
use std::ffi::CString;
use std::sync::mpsc::{self, SyncSender};
use std::time::Instant;

// VIDIOC_G_CTRL = _IOWR('V', 27, struct v4l2_control) = 0xC008_561B
// VIDIOC_S_CTRL = _IOWR('V', 28, struct v4l2_control) = 0xC008_561C
const VIDIOC_G_CTRL: libc::c_ulong = 0xC008_561B;
const VIDIOC_S_CTRL: libc::c_ulong = 0xC008_561C;

#[repr(C)]
struct V4l2CtrlReq {
    id: u32,
    value: i32,
}

enum CtrlMsg {
    Set { cid: u32, value: i32 },
}

pub struct V4l2Control {
    pub name: &'static str,
    pub label: &'static str,
    pub min: i32,
    pub max: i32,
    pub default: i32,
    pub value: i32,
}

pub struct V4l2Controls {
    pub ctrls: Vec<V4l2Control>,
    /// Non-blocking sender; the background thread owns the fd and processes ioctls.
    cmd_tx: SyncSender<CtrlMsg>,
}

// (name, label, min, max, default, V4L2_CID)
static DEFAULTS: &[(&str, &str, i32, i32, i32, u32)] = &[
    ("brightness", "Brightness", 0, 255, 25, 0x00980900),
    ("contrast", "Contrast", 0, 255, 127, 0x00980901),
    ("saturation", "Saturation", 0, 255, 127, 0x00980902),
    ("hue", "Hue", 0, 127, 0, 0x00980903),
    ("gamma", "Gamma", 0, 50, 0, 0x00980910),
];

impl V4l2Controls {
    pub fn new(device: &str) -> Self {
        // Open the fd here (startup, not yet streaming) so blocking is tolerable.
        let ctrl_fd = CString::new(device)
            .map(|c| unsafe { libc::open(c.as_ptr(), libc::O_RDWR) })
            .unwrap_or(-1);

        let mut ctrls: Vec<V4l2Control> = DEFAULTS
            .iter()
            .map(|(name, label, min, max, default, _)| V4l2Control {
                name,
                label,
                min: *min,
                max: *max,
                default: *default,
                value: *default,
            })
            .collect();

        // Read current hardware values synchronously at startup (acceptable one-time cost).
        if ctrl_fd >= 0 {
            for (ctrl, (_, _, _, _, _, cid)) in ctrls.iter_mut().zip(DEFAULTS) {
                let mut req = V4l2CtrlReq { id: *cid, value: 0 };
                if unsafe { libc::ioctl(ctrl_fd, VIDIOC_G_CTRL, &mut req) } == 0 {
                    ctrl.value = req.value.clamp(ctrl.min, ctrl.max);
                }
            }
        }

        // Bound-1 sync channel: the background thread processes one ioctl at a time;
        // if it's slow, we silently drop older values (try_send) so the UI never blocks.
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<CtrlMsg>(16);

        // Background thread owns ctrl_fd and issues all VIDIOC_S_CTRL calls.
        // If a USB control transfer blocks for several seconds, it only blocks this
        // thread, not the egui UI thread. Logs slow ioctls to stderr.
        std::thread::spawn(move || {
            for msg in cmd_rx {
                match msg {
                    CtrlMsg::Set { cid, value } => {
                        if ctrl_fd < 0 {
                            continue;
                        }
                        let req = V4l2CtrlReq { id: cid, value };
                        let t0 = Instant::now();
                        let ret = unsafe {
                            libc::ioctl(ctrl_fd, VIDIOC_S_CTRL, &req as *const V4l2CtrlReq)
                        };
                        let ms = t0.elapsed().as_millis();
                        if ret != 0 || ms > 50 {
                            let errno = unsafe { *libc::__errno_location() };
                            eprintln!(
                                "v4l2: VIDIOC_S_CTRL cid={cid:#010x}={value} ret={ret} errno={errno} took {ms}ms"
                            );
                        }
                    }
                }
            }
            // Sender dropped (V4l2Controls destroyed): close fd.
            if ctrl_fd >= 0 {
                unsafe {
                    libc::close(ctrl_fd);
                }
            }
        });

        Self { ctrls, cmd_tx }
    }

    /// Snapshot current control values as a name→value map for persistence.
    pub fn to_preset(&self) -> BTreeMap<String, i32> {
        self.ctrls
            .iter()
            .map(|c| (c.name.to_owned(), c.value))
            .collect()
    }

    /// Apply a name→value map to controls and queue VIDIOC_S_CTRL for each.
    pub fn apply_values(&mut self, preset: &BTreeMap<String, i32>) {
        let mut fires: Vec<(u32, i32)> = Vec::new();
        for (ctrl, (_, _, _, _, _, cid)) in self.ctrls.iter_mut().zip(DEFAULTS) {
            if let Some(&v) = preset.get(ctrl.name) {
                ctrl.value = v.clamp(ctrl.min, ctrl.max);
                fires.push((*cid, ctrl.value));
            }
        }
        for (cid, value) in fires {
            self.send(CtrlMsg::Set { cid, value });
        }
    }

    /// Reset all controls to driver defaults and queue VIDIOC_S_CTRL for each.
    pub fn reset_all(&mut self) {
        let mut fires: Vec<(u32, i32)> = Vec::new();
        for (ctrl, (_, _, _, _, _, cid)) in self.ctrls.iter_mut().zip(DEFAULTS) {
            ctrl.value = ctrl.default;
            fires.push((*cid, ctrl.value));
        }
        for (cid, value) in fires {
            self.send(CtrlMsg::Set { cid, value });
        }
    }

    fn reset_one(&mut self, idx: usize) {
        let value = {
            let ctrl = &mut self.ctrls[idx];
            ctrl.value = ctrl.default;
            ctrl.value
        };
        let cid = DEFAULTS[idx].5;
        self.send(CtrlMsg::Set { cid, value });
    }

    /// Enqueue a control message. Uses try_send so the UI thread never blocks;
    /// if the channel is full (background thread is busy with a slow ioctl),
    /// the change is silently dropped — the next slider tick will re-send.
    fn send(&self, msg: CtrlMsg) {
        let _ = self.cmd_tx.try_send(msg);
    }

    /// Draw the 5-row slider panel.
    /// `show_close` — when true, renders a ◀ button that signals the caller to close the panel.
    /// Returns `(changed, close_clicked)`.
    pub fn show_panel(&mut self, ui: &mut egui::Ui, show_close: bool) -> (bool, bool) {
        let mut close_clicked = false;
        ui.horizontal(|ui| {
            ui.heading("Input");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if show_close && ui.small_button("◀").on_hover_text("Close panel").clicked() {
                    close_clicked = true;
                }
                if ui.small_button("Reset All").clicked() {
                    self.reset_all();
                }
            });
        });
        ui.separator();

        let mut fires: Vec<(u32, i32)> = Vec::new();
        let mut reset_idx: Option<usize> = None;

        egui::Grid::new("v4l2_sliders")
            .num_columns(3)
            .spacing([6.0, 4.0])
            .show(ui, |ui| {
                for (i, (ctrl, (_, _, _, _, _, cid))) in
                    self.ctrls.iter_mut().zip(DEFAULTS).enumerate()
                {
                    ui.label(ctrl.label);
                    let resp = ui.add(
                        egui::Slider::new(&mut ctrl.value, ctrl.min..=ctrl.max)
                            .clamping(egui::SliderClamping::Always),
                    );
                    if resp.changed() {
                        fires.push((*cid, ctrl.value));
                    }
                    if ui
                        .small_button("↺")
                        .on_hover_text(format!("Reset to {}", ctrl.default))
                        .clicked()
                    {
                        reset_idx = Some(i);
                    }
                    ui.end_row();
                }
            });

        let changed = !fires.is_empty() || reset_idx.is_some();
        for (cid, value) in fires {
            self.send(CtrlMsg::Set { cid, value });
        }
        if let Some(i) = reset_idx {
            self.reset_one(i);
        }
        (changed, close_clicked)
    }
}
