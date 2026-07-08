use std::ffi::{CString, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use glow::HasContext;
use libmpv2::Mpv;
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};

// ---------------------------------------------------------------------------
// Source type
// ---------------------------------------------------------------------------
#[derive(Clone, Debug)]
pub enum Source {
    File(std::path::PathBuf),
    /// A file that is actively being written (e.g. a live capture).
    /// Like File but with cache-pause=false so mpv goes idle (not paused)
    /// when it reaches the current write position, allowing EOF recovery.
    V4l2(String),
    Udp(String),
}

impl Source {
    pub fn to_mpv_url(&self) -> String {
        match self {
            Source::File(p) => p.to_string_lossy().into_owned(),
            Source::V4l2(dev) => format!("av://v4l2:{dev}"),
            Source::Udp(url) => url.clone(),
        }
    }
    pub fn is_live(&self) -> bool {
        matches!(self, Source::V4l2(_) | Source::Udp(_))
    }
}

// ---------------------------------------------------------------------------
// Blit resources shared with the PaintCallback closure.
// All GL objects are owned by and accessed only on the GL (main) thread.
// The Arc<BlitData> satisfies the Send+Sync requirement of egui PaintCallback.
// ---------------------------------------------------------------------------
pub struct BlitData {
    pub tex: glow::NativeTexture,
    pub program: glow::NativeProgram,
    pub vao: glow::NativeVertexArray,
    pub has_frame: AtomicBool,
}

// SAFETY: All fields are accessed only from the eframe GL thread.
unsafe impl Send for BlitData {}
unsafe impl Sync for BlitData {}

// ---------------------------------------------------------------------------
// Playback state polled from mpv each frame
// ---------------------------------------------------------------------------
#[derive(Clone, Debug, Default)]
pub struct PlaybackState {
    pub time_pos: f64,
    pub duration: f64,
    pub paused: bool,
    pub idle: bool,
}

// ---------------------------------------------------------------------------
// MpvView — owns the mpv handle, render context, and GL resources
// Lives entirely on the GL/main thread; NOT Send.
// ---------------------------------------------------------------------------
pub struct MpvView {
    mpv: &'static Mpv,
    render_ctx: RenderContext<'static>,
    fbo: glow::NativeFramebuffer,
    fbo_id: i32, // integer ID read back from GL, passed to mpv render
    fb_w: i32,
    fb_h: i32,
    blit: Arc<BlitData>,
    pub state: PlaybackState,
    shared_state: Arc<Mutex<PlaybackState>>,
    /// Tracks the active source so we know whether we are on a live stream.
    current_source: Option<Source>,
}

impl MpvView {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let gl = cc
            .gl
            .as_ref()
            .expect("eframe must be running with the glow backend");

        // --- Create mpv ---
        let mpv = Mpv::with_initializer(|init| {
            init.set_property("vo", "libmpv")?;
            init.set_property("log-file", "/tmp/vhs-gui-mpv.log")?;
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("mpv init: {e}"))?;
        // Leak so we get a 'static reference; this is a long-running app,
        // mpv lives for the process lifetime.
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));

        // --- Set low-latency defaults (can be overridden per-source) ---
        let _ = mpv.set_property("cache", "no");
        let _ = mpv.set_property("audio", "no"); // preview is video-only

        // --- GL resources ---
        let (fbo, fbo_id, tex) = unsafe { create_fbo(gl, 720, 480) };
        let (program, vao) = unsafe { create_blit_shader(gl) };

        // --- Render context ---
        let render_ctx = mpv
            .create_render_context(vec![
                RenderParam::ApiType(RenderParamApiType::OpenGl),
                RenderParam::InitParams(OpenGLInitParams {
                    get_proc_address,
                    ctx: (),
                }),
                RenderParam::AdvancedControl(true),
            ])
            .map_err(|e| anyhow::anyhow!("render context: {e}"))?;

        let blit = Arc::new(BlitData {
            tex,
            program,
            vao,
            has_frame: AtomicBool::new(false),
        });

        // Wire repaint callback
        {
            let egui_ctx = cc.egui_ctx.clone();
            // SAFETY: render_ctx is used only on GL thread; callback just requests repaint.
            let render_ctx_ptr = &render_ctx as *const RenderContext<'static> as usize;
            let _ = render_ctx_ptr; // not used; just wire callback via a different mechanism below
            let _ = egui_ctx;
        }

        // Register property observers — updates arrive via the event thread.
        let _ = mpv.observe_property("time-pos", libmpv2::Format::Double, 0);
        let _ = mpv.observe_property("duration", libmpv2::Format::Double, 1);
        let _ = mpv.observe_property("pause", libmpv2::Format::Flag, 2);
        let _ = mpv.observe_property("idle-active", libmpv2::Format::Flag, 3);

        let shared_state = Arc::new(Mutex::new(PlaybackState::default()));

        Ok(Self {
            mpv,
            render_ctx,
            fbo,
            fbo_id,
            fb_w: 720,
            fb_h: 480,
            blit,
            state: PlaybackState::default(),
            shared_state,
            current_source: None,
        })
    }

    /// Wire the repaint callback and spawn the event thread.
    /// Call after new() with the egui context.
    pub fn wire_repaint(&mut self, egui_ctx: egui::Context) {
        // set_update_callback takes &mut self so must happen after construction
        let repaint_ctx = egui_ctx.clone();
        self.render_ctx.set_update_callback(move || {
            egui_ctx.request_repaint();
        });

        // Event thread: drain mpv events and update shared playback state.
        // Uses observe_property registered in new(); no get_property on the GL thread.
        let mpv: &'static Mpv = self.mpv;
        let shared = Arc::clone(&self.shared_state);
        std::thread::spawn(move || {
            use libmpv2::events::{Event, PropertyData};
            loop {
                match mpv.wait_event(60.0) {
                    Some(Ok(Event::PropertyChange { name, change, .. })) => {
                        if let Ok(mut s) = shared.lock() {
                            match (name, change) {
                                ("time-pos", PropertyData::Double(v)) => s.time_pos = v,
                                ("duration", PropertyData::Double(v)) => s.duration = v,
                                ("pause", PropertyData::Flag(v)) => s.paused = v,
                                ("idle-active", PropertyData::Flag(v)) => {
                                    s.idle = v;
                                    repaint_ctx.request_repaint();
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Event::Shutdown)) => break,
                    // None = MPV_EVENT_NONE (timeout expired with no event) — keep looping.
                    None => {}
                    _ => {}
                }
            }
        });
    }

    // -----------------------------------------------------------------------
    // Source switching
    // -----------------------------------------------------------------------

    pub fn open(&mut self, src: &Source) {
        let url = src.to_mpv_url();
        if matches!(src, Source::V4l2(_)) {
            // Raw V4L2: untimed mode prevents burst-then-stall stutter from
            // the unclocked raw input; cap demuxer read-ahead accordingly.
            let _ = self.mpv.set_property("untimed", true);
            let _ = self.mpv.set_property("vd-lavc-threads", 1i64);
            let _ = self.mpv.set_property("demuxer-max-bytes", "200KiB");
            let _ = self.mpv.set_property("demuxer-max-back-bytes", "0");
            let _ = self.mpv.set_property("cache-pause", false);
            let _ = self.mpv.set_property("framedrop", "vo");
            let _ = self.mpv.set_property("demuxer-lavf-o", "fflags=nobuffer");
        } else if matches!(src, Source::Udp(_)) {
            // MPEG-TS UDP preview: encoded with proper PTS so use timed playback.
            // (mpv's native OSD doesn't composite through the custom FBO blit —
            // the capture timer is drawn as an egui overlay instead.)
            let _ = self.mpv.set_property("untimed", false);
            let _ = self.mpv.set_property("vd-lavc-threads", 0i64);
            let _ = self.mpv.set_property("demuxer-max-bytes", "200KiB");
            let _ = self.mpv.set_property("demuxer-max-back-bytes", "0");
            let _ = self.mpv.set_property("cache-pause", false);
            let _ = self.mpv.set_property("framedrop", "vo");
            let _ = self.mpv.set_property("demuxer-lavf-o", "fflags=nobuffer");
        } else {
            let _ = self.mpv.set_property("untimed", false);
            let _ = self.mpv.set_property("vd-lavc-threads", 0i64);
            let _ = self.mpv.set_property("demuxer-max-bytes", "150MiB");
            let _ = self.mpv.set_property("demuxer-max-back-bytes", "150MiB");
            let _ = self.mpv.set_property("cache-pause", true);
            let _ = self.mpv.set_property("framedrop", "vo");
            let _ = self.mpv.set_property("demuxer-lavf-o", "");
        }
        let _ = self.mpv.command("loadfile", &[&url, "replace"]);
        self.current_source = Some(src.clone());
    }

    /// Send stop command without blocking. The V4L2 fd is released once
    /// mpv becomes idle, which the event thread reports via `state.idle`.
    pub fn stop(&mut self) {
        let _ = self.mpv.command("stop", &[]);
        self.current_source = None;
    }

    // -----------------------------------------------------------------------
    // Playback controls
    // -----------------------------------------------------------------------

    pub fn toggle_pause(&self) {
        let _ = self.mpv.command("cycle", &["pause"]);
    }

    // -----------------------------------------------------------------------
    // Called at the TOP of App::update(), before any UI.
    // Renders the current mpv frame into our off-screen FBO.
    // -----------------------------------------------------------------------
    pub fn render_frame(&mut self, gl: &glow::Context) {
        use libmpv2::render::mpv_render_update;
        let update = self.render_ctx.update().unwrap_or(0);
        if update & mpv_render_update::Frame != 0 {
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            }
            let _ = self
                .render_ctx
                .render::<()>(self.fbo_id, self.fb_w, self.fb_h, false);
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
            self.blit.has_frame.store(true, Ordering::Relaxed);
        }

        // Copy latest state from the event thread (non-blocking)
        if let Ok(s) = self.shared_state.lock() {
            self.state = s.clone();
        }
    }

    // -----------------------------------------------------------------------
    // Show the video panel in egui.
    // -----------------------------------------------------------------------
    pub fn show(&self, ui: &mut egui::Ui, capture_osd: Option<&str>) {
        // Determine available space, keep 4:3 aspect
        let available = ui.available_size();
        let w = available.x;
        let h = (w * 3.0 / 4.0).min(available.y);
        let size = egui::vec2(w, h);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

        // Click toggles pause for file playback; live sources (V4L2) don't pause.
        let is_live = self
            .current_source
            .as_ref()
            .map(|s| s.is_live())
            .unwrap_or(false);
        if response.clicked() && !is_live && self.current_source.is_some() {
            self.toggle_pause();
        }

        // PaintCallback blit
        let blit = Arc::clone(&self.blit);
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                if !blit.has_frame.load(Ordering::Relaxed) {
                    return;
                }
                let gl = painter.gl();
                let ppp = info.pixels_per_point;

                // Compute GL viewport (Y flipped: GL origin is bottom-left)
                let screen_h = info.viewport_in_pixels().height_px;
                let x = (rect.min.x * ppp).round() as i32;
                let y_egui = (rect.min.y * ppp).round() as i32;
                let w = (rect.width() * ppp).round() as i32;
                let h = (rect.height() * ppp).round() as i32;
                let y_gl = screen_h - y_egui - h;

                unsafe {
                    gl.viewport(x, y_gl, w, h);
                    gl.disable(glow::DEPTH_TEST);
                    gl.disable(glow::BLEND);
                    gl.use_program(Some(blit.program));
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(blit.tex));
                    gl.uniform_1_i32(gl.get_uniform_location(blit.program, "u_tex").as_ref(), 0);
                    gl.bind_vertex_array(Some(blit.vao));
                    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                    gl.bind_vertex_array(None);
                    gl.use_program(None);
                    gl.bind_texture(glow::TEXTURE_2D, None);

                    // Restore full-window viewport and re-enable blend so egui
                    // can correctly render any shapes (OSD, text) after this callback.
                    let vp = info.viewport_in_pixels();
                    gl.viewport(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
                    gl.enable(glow::BLEND);
                }
            })),
        });

        let painter = ui.painter();

        // OSD — time played / remaining, overlaid at bottom of video.
        // Gate on source-is-file (not duration) so it appears even if mpv hasn't
        // reported duration yet.
        if !is_live && self.current_source.is_some() && !self.state.idle {
            let played = fmt_time(self.state.time_pos);
            let osd = if self.state.duration > 0.0 {
                let remaining = fmt_time(self.state.duration - self.state.time_pos);
                format!("{played}  /  -{remaining}")
            } else {
                played
            };

            let font = egui::FontId::monospace(15.0);
            let padding = egui::vec2(8.0, 4.0);

            let galley = painter.layout_no_wrap(osd.clone(), font.clone(), egui::Color32::WHITE);
            let ts = galley.size();
            // Bottom-centre, a few pixels above the seek bar
            let text_origin = egui::pos2(
                rect.center().x - ts.x / 2.0,
                rect.max.y - ts.y - padding.y * 2.0 - 6.0,
            );
            let bg = egui::Rect::from_min_size(text_origin - padding, ts + padding * 2.0);
            painter.rect_filled(bg, 4.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160));
            painter.text(
                text_origin,
                egui::Align2::LEFT_TOP,
                osd,
                font,
                egui::Color32::WHITE,
            );

            // Pause indicator — ⏸ centred on the video when paused
            if self.state.paused {
                let icon_font = egui::FontId::proportional(48.0);
                let icon_galley =
                    painter.layout_no_wrap("⏸".to_owned(), icon_font.clone(), egui::Color32::WHITE);
                let is = icon_galley.size();
                let icon_origin = rect.center() - is / 2.0;
                let icon_bg = egui::Rect::from_min_size(
                    icon_origin - egui::vec2(10.0, 6.0),
                    is + egui::vec2(20.0, 12.0),
                );
                painter.rect_filled(
                    icon_bg,
                    8.0,
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 140),
                );
                painter.text(
                    icon_origin,
                    egui::Align2::LEFT_TOP,
                    "⏸",
                    icon_font,
                    egui::Color32::WHITE,
                );
            }
        }

        // Capture timer overlay — wall-clock elapsed from CaptureController, drawn
        // regardless of is_live/idle so it always shows over the live UDP preview.
        // (mpv's native OSD doesn't survive the custom FBO→blit render path, so
        // this egui overlay is the only reliable way to display a timer.)
        if let Some(text) = capture_osd {
            let label = format!("\u{25CF} REC  {text}"); // ● REC  HH:MM:SS
            let font = egui::FontId::monospace(15.0);
            let padding = egui::vec2(8.0, 4.0);
            let galley = painter.layout_no_wrap(label.clone(), font.clone(), egui::Color32::WHITE);
            let ts = galley.size();
            let text_origin = egui::pos2(
                rect.center().x - ts.x / 2.0,
                rect.max.y - ts.y - padding.y * 2.0 - 6.0,
            );
            let bg = egui::Rect::from_min_size(text_origin - padding, ts + padding * 2.0);
            painter.rect_filled(bg, 4.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160));
            painter.text(
                text_origin,
                egui::Align2::LEFT_TOP,
                label,
                font,
                egui::Color32::from_rgb(255, 80, 80),
            );
        }

        // Seek bar below the video (only when duration is known)
        if self.state.duration > 0.0 {
            let frac = (self.state.time_pos / self.state.duration) as f32;
            let seek_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x, rect.max.y + 4.0),
                egui::vec2(rect.width(), 8.0),
            );
            painter.rect_filled(seek_rect, 0.0, egui::Color32::from_gray(50));
            let played_rect = egui::Rect::from_min_size(
                seek_rect.min,
                egui::vec2(seek_rect.width() * frac, seek_rect.height()),
            );
            painter.rect_filled(played_rect, 0.0, egui::Color32::from_rgb(200, 60, 60));
        }
    }
}

// ---------------------------------------------------------------------------
// Time formatting helper
// ---------------------------------------------------------------------------
fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sc = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{sc:02}")
    } else {
        format!("{m}:{sc:02}")
    }
}

// ---------------------------------------------------------------------------
// GL resource creation helpers
// ---------------------------------------------------------------------------

/// Returns (fbo, fbo_integer_id, color_texture)
unsafe fn create_fbo(
    gl: &glow::Context,
    w: i32,
    h: i32,
) -> (glow::NativeFramebuffer, i32, glow::NativeTexture) {
    unsafe {
        let tex = gl.create_texture().expect("create texture");
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGB as i32,
            w,
            h,
            0,
            glow::RGB,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.bind_texture(glow::TEXTURE_2D, None);

        let fbo = gl.create_framebuffer().expect("create framebuffer");
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(tex),
            0,
        );
        // Read back the integer FBO id that mpv needs
        let fbo_id = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        (fbo, fbo_id, tex)
    }
}

/// Compile the blit shader and create a VAO.
unsafe fn create_blit_shader(gl: &glow::Context) -> (glow::NativeProgram, glow::NativeVertexArray) {
    const VERT: &str = r#"#version 330 core
out vec2 v_tc;
void main() {
    // Generate a fullscreen triangle strip quad from gl_VertexID
    float x = float((gl_VertexID >> 1) & 1) * 2.0 - 1.0;
    float y = float(gl_VertexID & 1) * 2.0 - 1.0;
    v_tc = vec2(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    gl_Position = vec4(x, y, 0.0, 1.0);
}"#;
    const FRAG: &str = r#"#version 330 core
uniform sampler2D u_tex;
in vec2 v_tc;
out vec4 f_color;
void main() { f_color = texture(u_tex, v_tc); }"#;

    unsafe {
        let vert = gl.create_shader(glow::VERTEX_SHADER).unwrap();
        gl.shader_source(vert, VERT);
        gl.compile_shader(vert);
        assert!(
            gl.get_shader_compile_status(vert),
            "blit vert: {}",
            gl.get_shader_info_log(vert)
        );

        let frag = gl.create_shader(glow::FRAGMENT_SHADER).unwrap();
        gl.shader_source(frag, FRAG);
        gl.compile_shader(frag);
        assert!(
            gl.get_shader_compile_status(frag),
            "blit frag: {}",
            gl.get_shader_info_log(frag)
        );

        let program = gl.create_program().unwrap();
        gl.attach_shader(program, vert);
        gl.attach_shader(program, frag);
        gl.link_program(program);
        assert!(
            gl.get_program_link_status(program),
            "blit link: {}",
            gl.get_program_info_log(program)
        );
        gl.detach_shader(program, vert);
        gl.detach_shader(program, frag);
        gl.delete_shader(vert);
        gl.delete_shader(frag);

        let vao = gl.create_vertex_array().unwrap();
        (program, vao)
    }
}

// ---------------------------------------------------------------------------
// OpenGL symbol resolver for mpv (bare fn pointer — cannot capture)
// ---------------------------------------------------------------------------
fn get_proc_address(_ctx: &(), name: &str) -> *mut c_void {
    let cname = CString::new(name).unwrap();
    unsafe {
        let sym = libc::dlsym(libc::RTLD_DEFAULT, cname.as_ptr());
        if !sym.is_null() {
            return sym as *mut _;
        }
        egl_get_proc(cname.as_ptr())
    }
}

unsafe fn egl_get_proc(name: *const libc::c_char) -> *mut c_void {
    use std::sync::OnceLock;
    type EglGetProcFn = unsafe extern "C" fn(*const libc::c_char) -> *mut c_void;
    static FN: OnceLock<EglGetProcFn> = OnceLock::new();
    let f = FN.get_or_init(|| {
        let sym = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"eglGetProcAddress".as_ptr()) };
        if sym.is_null() {
            panic!("eglGetProcAddress not found — is eframe using EGL?");
        }
        unsafe { std::mem::transmute(sym) }
    });
    unsafe { f(name) }
}
