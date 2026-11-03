mod app;
mod audio_cleanup;
mod capture;
mod clock;
mod config;
mod denoise;
mod fix_sync;
mod library;
mod monitor_feed;
mod mpv_view;
mod panels;
mod persist;
mod pipeline;
mod settings;
mod tools;
mod upscale;
mod v4l2;
mod viewer_encode;
mod vs_assets;
mod vs_ops;
mod vs_stack;

/// Decode the embedded icon PNG and return `egui::IconData`.
/// Returns `None` on any decode error so startup isn't blocked.
fn load_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/icon.png");
    let decoder = png::Decoder::new(bytes.as_slice());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let bytes = &buf[..info.buffer_size()];

    // Convert to RGBA if necessary.
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => bytes.to_vec(),
        png::ColorType::Rgb => bytes
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        _ => return None,
    };

    Some(egui::IconData {
        rgba,
        width: info.width,
        height: info.height,
    })
}

fn main() -> eframe::Result {
    // `vhs-gui upscale-worker JOB.toml` runs an upscale job with no GUI at all; it must be decided
    // before any window or GL setup, and no other argument may change what plain `vhs-gui` does.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some(upscale::worker::SUBCOMMAND) {
        let rest: Vec<String> = args.collect();
        std::process::exit(upscale::worker::main(&rest));
    }

    let icon = load_icon();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("vhs-gui")
        .with_inner_size([720.0 + 224.0, 480.0 + 36.0]); // 720×480 raw capture pixels + side panel + toolbar
    if let Some(icon_data) = icon {
        viewport = viewport.with_icon(std::sync::Arc::new(icon_data));
    }

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "vhs-gui",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)?))),
    )
}
