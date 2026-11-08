//! Window and event-loop runner.
//!
//! Draws only when input arrives or when the UI asks for another frame
//! (animation, playback), so an idle window uses no CPU or GPU time.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::geom::Rect;
use crate::render::Renderer;
use crate::ui::Ui;

/// Application callbacks.
pub trait App {
    /// Builds one frame.
    fn update(&mut self, ui: &mut Ui, window: &WindowControl);
    /// Called when the user asks to close the window. Return false to keep
    /// it open (for example to show an unsaved-changes prompt).
    fn close_requested(&mut self) -> bool {
        true
    }
    /// Called once before the process exits.
    fn shutdown(&mut self) {}
}

/// Window operations available to the app during a frame.
pub struct WindowControl {
    window: Option<Arc<Window>>,
    title: std::cell::RefCell<String>,
    drag: std::cell::Cell<bool>,
    exit: std::cell::Cell<bool>,
    adapter: String,
}

impl WindowControl {
    /// A control with no window, for driving an [`App`] in tests.
    pub fn headless() -> WindowControl {
        WindowControl {
            window: None,
            title: std::cell::RefCell::new(String::new()),
            drag: std::cell::Cell::new(false),
            exit: std::cell::Cell::new(false),
            adapter: "headless".into(),
        }
    }

    pub fn title(&self) -> String {
        self.title.borrow().clone()
    }

    pub fn exit_requested(&self) -> bool {
        self.exit.get()
    }

    pub fn set_title(&self, title: &str) {
        if *self.title.borrow() != title {
            if let Some(w) = &self.window {
                w.set_title(title);
            }
            *self.title.borrow_mut() = title.to_string();
        }
    }

    /// Starts moving the window with the pointer (for custom title bars).
    pub fn drag_window(&self) {
        self.drag.set(true);
    }

    pub fn exit(&self) {
        self.exit.set(true);
    }

    /// Space reserved at the top-left for native window controls when the
    /// content extends into the title bar.
    pub fn titlebar_inset(&self) -> f32 {
        if cfg!(target_os = "macos") { 78.0 } else { 0.0 }
    }

    pub fn adapter_info(&self) -> &str {
        &self.adapter
    }

    pub fn is_maximized(&self) -> bool {
        self.window.as_ref().is_some_and(|w| w.is_maximized())
    }

    pub fn toggle_maximized(&self) {
        if let Some(w) = &self.window {
            w.set_maximized(!w.is_maximized());
        }
    }
}

pub struct WindowOptions {
    pub title: String,
    pub size: (f64, f64),
    pub min_size: (f64, f64),
    /// PNG bytes for the window/taskbar icon (Windows and Linux; macOS uses
    /// the bundle icon).
    pub icon_png: Option<&'static [u8]>,
}

impl Default for WindowOptions {
    fn default() -> Self {
        WindowOptions {
            title: "Peel Studio".into(),
            size: (1440.0, 900.0),
            min_size: (1200.0, 720.0),
            icon_png: None,
        }
    }
}

struct Running<A> {
    window: Arc<Window>,
    renderer: Renderer,
    control: WindowControl,
    app: A,
}

struct Runner<A, F> {
    options: WindowOptions,
    frames: u64,
    screenshot: Option<(std::path::PathBuf, u64)>,
    make: Option<F>,
    running: Option<Running<A>>,
    ui: Ui,
    start: Instant,
    error: Option<String>,
}

/// Opens the main window and runs until it closes. `make` builds the app
/// once the window and UI exist.
pub fn run<A: App + 'static, F: FnOnce(&mut Ui) -> A + 'static>(
    options: WindowOptions,
    make: F,
) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let screenshot = std::env::var_os("PEEL_SCREENSHOT").map(|p| {
        let frames = std::env::var("PEEL_SCREENSHOT_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8);
        (std::path::PathBuf::from(p), frames)
    });
    let mut options = options;
    if let Some((w, h)) = std::env::var("PEEL_WINDOW_SIZE").ok().and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    }) {
        options.size = (w, h);
    }
    let mut runner = Runner {
        options,
        frames: 0,
        screenshot,
        make: Some(make),
        running: None,
        ui: Ui::new(),
        start: Instant::now(),
        error: None,
    };
    event_loop.run_app(&mut runner).map_err(|e| e.to_string())?;
    match runner.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

impl<A: App, F: FnOnce(&mut Ui) -> A> ApplicationHandler for Runner<A, F> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        #[allow(unused_mut)]
        let mut attrs = Window::default_attributes()
            .with_title(&self.options.title)
            .with_inner_size(LogicalSize::new(self.options.size.0, self.options.size.1))
            .with_min_inner_size(LogicalSize::new(
                self.options.min_size.0,
                self.options.min_size.1,
            ));
        if let Some(icon) = self.options.icon_png.and_then(decode_icon) {
            attrs = attrs.with_window_icon(Some(icon));
        }
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(format!("could not create window: {e}"));
                event_loop.exit();
                return;
            }
        };
        let renderer = match pollster::block_on(Renderer::new(
            window.clone(),
            event_loop.owned_display_handle(),
        )) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(e.to_string());
                event_loop.exit();
                return;
            }
        };
        log::info!("graphics adapter: {}", renderer.adapter_info());
        let control = WindowControl {
            window: Some(window.clone()),
            title: std::cell::RefCell::new(self.options.title.clone()),
            drag: std::cell::Cell::new(false),
            exit: std::cell::Cell::new(false),
            adapter: renderer.adapter_info().to_string(),
        };
        let make = self.make.take().expect("app factory");
        let app = make(&mut self.ui);
        self.running = Some(Running {
            window: window.clone(),
            renderer,
            control,
            app,
        });
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(run) = &mut self.running else { return };
        let now = self.start.elapsed().as_secs_f64();
        match &event {
            WindowEvent::CloseRequested => {
                if run.app.close_requested() {
                    run.app.shutdown();
                    event_loop.exit();
                }
                return;
            }
            WindowEvent::Resized(size) => {
                run.renderer.resize(size.width, size.height);
                run.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => run.window.request_redraw(),
            WindowEvent::RedrawRequested => {
                self.frame(event_loop, now);
                return;
            }
            _ => {}
        }
        self.ui.input.set_scale(run.window.scale_factor() as f32);
        self.ui.input.handle(&event, now);
        run.window.request_redraw();
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {}

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(run) = &mut self.running {
            run.app.shutdown();
        }
    }
}

impl<A: App, F: FnOnce(&mut Ui) -> A> Runner<A, F> {
    fn frame(&mut self, event_loop: &ActiveEventLoop, now: f64) {
        let Some(run) = &mut self.running else { return };
        let scale = run.window.scale_factor() as f32;
        let size = run.window.inner_size();
        let screen = Rect {
            x: 0.0,
            y: 0.0,
            w: size.width as f32 / scale,
            h: size.height as f32 / scale,
        };
        self.ui.begin_frame(screen, scale, now);
        run.app.update(&mut self.ui, &run.control);
        self.ui.end_frame();
        run.window.set_cursor(self.ui.cursor());
        let clear = self.ui.theme.bg;
        self.frames += 1;
        if let Some((path, at)) = &self.screenshot
            && self.frames >= *at
        {
            let (w, h, pixels) = run
                .renderer
                .capture(&self.ui.scene, &mut self.ui.text, clear);
            match write_png(path, w, h, &pixels) {
                Ok(()) => log::info!("screenshot written to {}", path.display()),
                Err(e) => log::error!("screenshot failed: {e}"),
            }
            run.app.shutdown();
            event_loop.exit();
            return;
        }
        let ok = run
            .renderer
            .render(&self.ui.scene, &mut self.ui.text, clear);
        if run.control.drag.replace(false)
            && let Err(e) = run.window.drag_window()
        {
            log::debug!("drag_window: {e}");
        }
        if run.control.exit.get() {
            run.app.shutdown();
            event_loop.exit();
            return;
        }
        if !ok || self.ui.wants_repaint() || self.screenshot.is_some() {
            run.window.request_redraw();
        }
    }
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())
}

fn decode_icon(png_bytes: &[u8]) -> Option<winit::window::Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    winit::window::Icon::from_rgba(buf, info.width, info.height).ok()
}
