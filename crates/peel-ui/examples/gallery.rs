//! Widget gallery: `cargo run -p peel-ui --example gallery`.

use peel_ui::*;

struct Gallery {
    knobs: [f32; 4],
    fader: f32,
    slider: f32,
    seg: usize,
    bpm: f64,
    name: String,
    led: bool,
}

impl App for Gallery {
    fn update(&mut self, ui: &mut Ui, _w: &WindowControl) {
        let t = ui.theme.clone();
        let screen = ui.screen();
        ui.scene.rect(screen, 0.0, t.bg);
        let mut area = screen.shrink(24.0);
        area.cut_top(24.0);
        let mut panel = area.cut_left(560.0);
        ui.scene.rect(panel, t.radius_l, t.panel);
        panel = panel.shrink(16.0);
        let head = panel.cut_top(28.0);
        ui.text(head, "Peel Studio widgets", t.heading, t.text, Align::Start);
        panel.cut_top(8.0);
        let mut row = panel.cut_top(32.0);
        ui.button(Id::new("b1"), row.cut_left(96.0), "Button");
        row.cut_left(8.0);
        ui.button_primary(Id::new("b2"), row.cut_left(96.0), "Export");
        row.cut_left(8.0);
        ui.icon_button_fill(
            Id::new("play"),
            row.cut_left(32.0),
            icons::PLAY,
            true,
            t.play,
            16.0,
        );
        row.cut_left(4.0);
        ui.icon_button_fill(
            Id::new("stop"),
            row.cut_left(32.0),
            icons::STOP,
            false,
            t.text,
            14.0,
        );
        row.cut_left(4.0);
        ui.icon_button_fill(
            Id::new("rec"),
            row.cut_left(32.0),
            icons::RECORD,
            false,
            t.record,
            14.0,
        );
        row.cut_left(8.0);
        let led = ui.led(Id::new("led"), row.cut_left(24.0), self.led, t.play);
        ui.name(&led, "Monitoring");
        if led.clicked {
            self.led = !self.led;
        }
        panel.cut_top(12.0);
        let seg = panel.cut_top(28.0).with_w(220.0);
        ui.segmented(Id::new("seg"), seg, &["Pattern", "Song"], &mut self.seg);
        panel.cut_top(16.0);
        let mut knobs = panel.cut_top(64.0);
        let colors = [
            peel_ui::Color::hex(0xEC5E48),
            t.accent,
            peel_ui::Color::hex(0x55C48E),
            peel_ui::Color::hex(0x5E9FEE),
        ];
        for i in 0..4 {
            let mut k = knobs.cut_left(56.0);
            let label = k.cut_bottom(16.0);
            let name = ["Cutoff", "Res", "Drive", "Pan"][i];
            let knob = ui.knob(
                Id::new(("knob", i)),
                k,
                &mut self.knobs[i],
                0.5,
                colors[i],
                i == 3,
            );
            // Screen readers cannot see the caption below the knob.
            ui.name(&knob, name);
            ui.text(label, name, t.caption, t.text_faint, Align::Center);
            knobs.cut_left(12.0);
        }
        panel.cut_top(16.0);
        let mut s = panel.cut_top(24.0).with_w(240.0);
        ui.hslider(
            Id::new("sl"),
            s.cut_left(240.0),
            &mut self.slider,
            0.5,
            t.accent,
        );
        ui.label_last("Mix");
        panel.cut_top(16.0);
        let bpm = panel.cut_top(36.0).with_w(110.0);
        ui.scene.rect(bpm, t.radius_m, t.bg);
        ui.drag_number(
            Id::new("bpm"),
            bpm,
            &mut self.bpm,
            20.0,
            999.0,
            0.25,
            t.readout,
            t.accent,
            |v| format!("{v:.1}"),
        );
        ui.label_last("Tempo");
        panel.cut_top(16.0);
        let name = panel.cut_top(30.0).with_w(260.0);
        ui.text_edit(Id::new("name"), name, &mut self.name, "Search sounds");
        panel.cut_top(16.0);
        let tri = panel.cut_top(40.0);
        ui.scene.triangle(
            Rect {
                x: tri.x,
                y: tri.y,
                w: 30.0,
                h: 34.0,
            },
            Dir::Right,
            1.0,
            t.play,
        );
        ui.scene.arc(
            vec2(tri.x + 80.0, tri.y + 18.0),
            14.0,
            3.0,
            0.0,
            std::f32::consts::TAU,
            t.focus,
        );
        ui.scene.line(
            vec2(tri.x + 120.0, tri.y + 30.0),
            vec2(tri.x + 200.0, tri.y + 6.0),
            2.0,
            t.record,
        );
        ui.text_at(
            vec2(tri.x + 220.0, tri.y + 8.0),
            "1:03:240  128.00 BPM  -6.0 dB",
            t.readout_small,
            t.text_dim,
        );

        area.cut_left(24.0);
        let mut strip = area.cut_left(96.0).with_h(420.0);
        ui.scene.rect(strip, t.radius_l, t.panel);
        strip = strip.shrink(10.0);
        let label = strip.cut_top(20.0);
        ui.text(label, "Master", t.label, t.text, Align::Center);
        let mut body = strip;
        let meter = body.cut_right(14.0);
        ui.fader(Id::new("fader"), body, &mut self.fader, 0.75, t.accent);
        ui.label_last("Master volume");
        let lvl = (ui.time() as f32 * 1.7).sin().abs() * 1.05;
        ui.meter(Id::new("meter"), meter, (lvl, lvl * 0.8));
        ui.label_last("Master level");
        ui.request_repaint();
    }
}

fn main() {
    env_logger::init();
    peel_ui::run(WindowOptions::default(), |_| Gallery {
        knobs: [0.3, 0.6, 0.15, 0.5],
        fader: 0.75,
        slider: 0.4,
        seg: 0,
        bpm: 128.0,
        name: String::new(),
        led: true,
    })
    .unwrap();
}
