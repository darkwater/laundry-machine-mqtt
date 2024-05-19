use std::{
    cmp::Ordering,
    thread,
    time::{Duration, Instant},
};

use eframe::{egui, CreationContext};
use egui::{
    ahash::HashMap, load::ImagePoll, pos2, vec2, CentralPanel, Color32, Context, DragValue, Grid,
    Key, Pos2, Rect, Sense, SizeHint, Slider, Stroke, TextEdit, ViewportCommand, Widget, Window,
};
use rumqttc::MqttOptions;
use serde_json::Value;

use self::config::{Marker, MarkerType};

mod config;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Laundry Machine MQTT",
        native_options,
        Box::new(|cc| Box::new(MyEguiApp::new(cc))),
    )
}

struct MyEguiApp {
    config: config::Config,
    editing_marker: Option<usize>,
    image_refreshed: Instant,
    refresh_rate: Duration,
    sampled: Vec<Vec<f32>>,
    values: Vec<Value>,
}

impl MyEguiApp {
    fn new(cc: &CreationContext<'_>) -> Self {
        Self {
            config: cc
                .storage
                .and_then(|storage| eframe::get_value(storage, "config"))
                .unwrap_or_default(),
            editing_marker: None,
            image_refreshed: Instant::now(),
            refresh_rate: Duration::from_secs(15),
            sampled: vec![],
            values: vec![],
        }
    }
}

impl eframe::App for MyEguiApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        egui_extras::install_image_loaders(ctx);

        CentralPanel::default()
            .frame(egui::Frame::default().inner_margin(0.))
            .show(ctx, |ui| {
                let response = ui.image(&self.config.webcam.url);
                let rect = response.rect;

                let drag_response = ui.allocate_rect(rect, Sense::drag());
                let drag = drag_response.drag_delta();
                let mouse_pos = drag_response.interact_pointer_pos().unwrap_or_default();

                for (idx, marker) in self.config.markers.iter_mut().enumerate() {
                    match &mut marker.ty {
                        MarkerType::Point { pos, .. } => {
                            if self.editing_marker == Some(idx) {
                                pos.x += drag.x / rect.width();
                                pos.y += drag.y / rect.height();
                            }
                        }
                        MarkerType::SevenSegment {
                            start, end, bottom, ..
                        } => {
                            if self.editing_marker == Some(idx) {
                                let closest = [&mut *start, &mut *end, &mut *bottom]
                                    .into_iter()
                                    .min_by(|a, b| {
                                        let a_dist = (map_pos(**a, rect) - mouse_pos).length();
                                        let b_dist = (map_pos(**b, rect) - mouse_pos).length();
                                        a_dist.partial_cmp(&b_dist).unwrap_or(Ordering::Equal)
                                    })
                                    .unwrap();

                                closest.x += drag.x / rect.width();
                                closest.y += drag.y / rect.height();
                            }

                            let painter = ui.painter();

                            painter.line_segment(
                                [map_pos(*start, rect), map_pos(*end, rect)],
                                Stroke::new(0.2, Color32::WHITE),
                            );

                            painter.line_segment(
                                [map_pos(*start, rect), map_pos(*bottom, rect)],
                                Stroke::new(0.2, Color32::WHITE),
                            );

                            painter.circle_filled(map_pos(*start, rect), 2., Color32::RED);
                            painter.circle_filled(map_pos(*end, rect), 2., Color32::GREEN);
                            painter.circle_filled(map_pos(*bottom, rect), 2., Color32::BLUE);
                        }
                    }

                    let points = marker.ty.get_points();
                    for (pidx, point) in points.into_iter().enumerate() {
                        ui.painter().rect_stroke(
                            Rect::from_center_size(
                                map_pos(point.pos, rect),
                                rect.size() * point.size,
                            ),
                            0.,
                            Stroke::new(1., Color32::WHITE),
                        );

                        if let Some(sample) = self.sampled.get(idx).and_then(|v| v.get(pidx)) {
                            ui.painter().rect_filled(
                                Rect::from_center_size(map_pos(point.pos, rect), vec2(5., 5.)),
                                5.,
                                if sample > &self.config.luminance_threshold {
                                    Color32::WHITE
                                } else {
                                    Color32::BLACK
                                },
                            );
                        }
                    }
                }
            });

        Window::new("Options").show(ctx, |ui| {
            ui.set_min_width(100.);

            ui.collapsing("Webcam", |ui| {
                Grid::new("webcam_config").num_columns(2).show(ui, |ui| {
                    ui.label("URL");
                    ui.text_edit_singleline(&mut self.config.webcam.url);
                    ui.end_row();
                });

                if ui.button("Refresh").clicked() {
                    ctx.forget_image(&self.config.webcam.url);
                }
            });

            ui.collapsing("MQTT", |ui| {
                Grid::new("mqtt_config").num_columns(2).show(ui, |ui| {
                    ui.label("Host");
                    ui.text_edit_singleline(&mut self.config.mqtt.host);
                    ui.end_row();

                    ui.label("Port");
                    DragValue::new(&mut self.config.mqtt.port)
                        .speed(1)
                        .clamp_range(1..=65535)
                        .ui(ui);
                    ui.end_row();

                    fn opt(
                        ui: &mut egui::Ui,
                        value: &mut Option<String>,
                        label: &str,
                        password: bool,
                    ) {
                        ui.label(label);
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut value.is_some(), "").changed() {
                                if value.is_none() {
                                    *value = Some(String::new());
                                } else {
                                    *value = None;
                                }
                            }
                            if let Some(value) = value {
                                if password {
                                    TextEdit::singleline(value).password(true).show(ui);
                                } else {
                                    ui.text_edit_singleline(value);
                                }
                            }
                        });
                        ui.end_row();
                    }

                    opt(ui, &mut self.config.mqtt.username, "Username", false);
                    opt(ui, &mut self.config.mqtt.password, "Password", true);
                });

                if ui.button("Publish").clicked() {
                    self.publish();
                }
            });

            ui.collapsing("Markers", |ui| {
                let mut remove = None;

                for (idx, marker) in self.config.markers.iter_mut().enumerate() {
                    if let Some(value) = self.values.get(idx) {
                        ui.heading(serde_json::to_string(value).unwrap());
                    }

                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut marker.name);

                        if ui.button("Remove").clicked() {
                            remove = Some(idx);
                        }

                        if ui
                            .selectable_value(&mut self.editing_marker, Some(idx), "Edit")
                            .clicked()
                        {
                            self.editing_marker = Some(idx);
                        }
                    });

                    match &mut marker.ty {
                        MarkerType::Point { size, .. } => {
                            Slider::new(size, 0.001..=0.1).ui(ui);
                        }
                        MarkerType::SevenSegment {
                            digits,
                            spacing,
                            size,
                            ..
                        } => {
                            DragValue::new(digits).speed(0.1).clamp_range(1..=10).ui(ui);
                            Slider::new(spacing, 0.001..=0.1).ui(ui);
                            Slider::new(size, 0.001..=0.1).ui(ui);
                        }
                    }

                    ui.separator();
                }

                if let Some(remove) = remove {
                    self.config.markers.remove(remove);
                }

                if ui.button("Add point marker").clicked() {
                    self.config.markers.push(Marker::new(MarkerType::Point {
                        pos: Pos2::new(0.5, 0.5),
                        size: 0.01,
                    }));
                }

                if ui.button("Add seven segment marker").clicked() {
                    self.config
                        .markers
                        .push(Marker::new(MarkerType::SevenSegment {
                            start: Pos2::new(0.4, 0.4),
                            end: Pos2::new(0.4, 0.6),
                            bottom: Pos2::new(0.4, 0.5),
                            digits: 3,
                            spacing: 0.005,
                            size: 0.01,
                        }));
                }
            });

            ui.collapsing("Sampling", |ui| {
                Slider::new(&mut self.config.luminance_threshold, 0.001..=0.999).ui(ui);

                if ui.button("Sample").clicked() {
                    self.sample(ctx);
                }
            });
        });

        if self.image_refreshed.elapsed() > self.refresh_rate {
            self.sample(ctx);

            self.image_refreshed = Instant::now();
            ctx.forget_image(&self.config.webcam.url);
        }

        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "config", &self.config);
    }
}

impl MyEguiApp {
    fn sample(&mut self, ctx: &Context) {
        let image = ctx.try_load_image(&self.config.webcam.url, SizeHint::Width(100));
        if let Ok(ImagePoll::Ready { image }) = image {
            self.sampled = self
                .config
                .markers
                .iter()
                .map(|marker| {
                    marker
                        .ty
                        .get_points()
                        .into_iter()
                        .map(|point| {
                            let [r, g, b, _] = point
                                .sample(&image.pixels, image.width(), image.height())
                                .to_srgba_unmultiplied();

                            let r = r as f32 / 255.;
                            let g = g as f32 / 255.;
                            let b = b as f32 / 255.;

                            0.2126 * r + 0.7152 * g + 0.0722 * b
                        })
                        .collect::<Vec<_>>()
                })
                .collect();

            self.values = self
                .config
                .markers
                .iter()
                .enumerate()
                .map(|(idx, marker)| {
                    marker
                        .ty
                        .value(&self.sampled[idx], self.config.luminance_threshold)
                })
                .collect();

            self.publish();
        }
    }

    fn publish(&self) {
        let mut mqttoptions = MqttOptions::new(
            "laundry-machine-mqtt",
            &self.config.mqtt.host,
            self.config.mqtt.port,
        );
        mqttoptions.set_keep_alive(Duration::from_secs(5));

        let (client, mut connection) = rumqttc::Client::new(mqttoptions, 10);

        thread::spawn(move || {
            let start = Instant::now();
            let deadline = start + Duration::from_secs(2);
            while Instant::now() < deadline {
                let res = connection.recv_timeout(deadline.duration_since(Instant::now()));
                dbg!(res).ok();
            }
        });

        let mut values = self
            .config
            .markers
            .iter()
            .zip(&self.values)
            .map(|(marker, value)| (marker.name.as_str(), value))
            .collect::<HashMap<&str, &Value>>();

        if let (Some(Value::Number(hour)), Some(Value::Number(minute))) =
            (values.remove("hour"), values.remove("minute"))
        {
            if let (Some(hour), Some(minute)) = (hour.as_u64(), minute.as_u64()) {
                let minutes = hour * 60 + minute;
                let seconds = minutes * 60;

                match client.publish(
                    "laundry-machine/time-remaining",
                    rumqttc::QoS::AtLeastOnce,
                    false,
                    seconds.to_string(),
                ) {
                    Ok(()) => {
                        println!("Published time remaining: {} minutes", minutes);
                    }
                    Err(e) => {
                        eprintln!("Error publishing time remaining: {}", e);
                    }
                }
            }
        }

        for (name, value) in values {
            match client.publish(
                &format!("laundry-machine/{}", name),
                rumqttc::QoS::AtLeastOnce,
                false,
                serde_json::to_string_pretty(value).unwrap(),
            ) {
                Ok(()) => {
                    println!("Published {}: {}", name, value);
                }
                Err(e) => {
                    eprintln!("Error publishing {}: {}", name, e);
                }
            }
        }
    }
}

fn map_pos(normalized: Pos2, rect: Rect) -> Pos2 {
    pos2(
        rect.left() + rect.width() * normalized.x,
        rect.top() + rect.height() * normalized.y,
    )
}
