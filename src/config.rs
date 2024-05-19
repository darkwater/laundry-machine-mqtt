use egui::Pos2;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub mqtt: MqttConfig,
    #[serde(default)]
    pub webcam: WebcamConfig,
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default = "default_luminance_threshold")]
    pub luminance_threshold: f32,
}

fn default_luminance_threshold() -> f32 {
    0.4
}

#[derive(Default, Serialize, Deserialize)]
pub struct MqttConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct WebcamConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Marker {
    pub name: String,
    pub ty: MarkerType,
}

impl Marker {
    pub fn new(ty: MarkerType) -> Self {
        Self {
            name: Default::default(),
            ty,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum MarkerType {
    Point {
        pos: Pos2,
        size: f32,
    },
    SevenSegment {
        start: Pos2,
        end: Pos2,
        bottom: Pos2,
        digits: usize,
        spacing: f32,
        size: f32,
    },
}

pub struct Point {
    pub pos: Pos2,
    pub size: f32,
}

impl Point {
    pub fn sample<T: Copy>(&self, pixels: &[T], width: usize, height: usize) -> T {
        let x = (self.pos.x * width as f32).round() as usize;
        let y = (self.pos.y * height as f32).round() as usize;

        pixels[y * width + x]
    }
}

impl MarkerType {
    pub fn get_points(&self) -> Vec<Point> {
        match *self {
            MarkerType::Point { pos, size } => vec![Point { pos, size }],
            MarkerType::SevenSegment {
                start,
                end,
                bottom,
                digits,
                spacing,
                size,
            } => {
                let length = (end - start).length();
                let direction = (end - start).normalized();
                let tangent = bottom - start;

                let segment_length = (length - spacing * (digits as f32 - 1.)) / digits as f32;

                //  aa
                // f  b
                //  gg
                // e  c
                //  dd

                (0..digits)
                    .flat_map(|n| {
                        let start = start + direction * (n as f32 * (segment_length + spacing));
                        let center = start + direction * segment_length / 2.;
                        let end = start + direction * segment_length;

                        let a = center - tangent;
                        let b = end - tangent / 2.;
                        let c = end + tangent / 2.;
                        let d = center + tangent;
                        let e = start + tangent / 2.;
                        let f = start - tangent / 2.;
                        let g = center;

                        vec![a, b, c, d, e, f, g]
                    })
                    .map(|pos| Point { pos, size })
                    .collect()
            }
        }
    }

    pub fn value(&self, samples: &[f32], mut threshold: f32) -> serde_json::Value {
        match self {
            MarkerType::Point { .. } => {
                let Some(value) = samples.first() else {
                    return Value::Null;
                };

                Value::Bool(*value > threshold)
            }
            MarkerType::SevenSegment { .. } => {
                let mut threshold_change = 0.01;

                loop {
                    let number = samples
                        .chunks(7)
                        .map(|segment| {
                            seven_segment_to_number(
                                &segment
                                    .iter()
                                    .map(|&value| value > threshold)
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect::<Option<Vec<_>>>()
                        .map(|digits| {
                            digits
                                .iter()
                                .fold(0i32, |acc, value| acc * 10 + value)
                                .into()
                        });

                    if let Some(number) = number {
                        return Value::Number(number);
                    }

                    threshold += threshold_change;
                    threshold_change *= -1.5;

                    if !(0.0..=1.0).contains(&threshold) {
                        return Value::Null;
                    }
                }
            }
        }
    }
}

//  aa
// f  b
//  gg
// e  c
//  dd

fn seven_segment_to_number(segments: &[bool]) -> Option<i32> {
    match segments {
        // aa   bb    cc    dd    ee     ff     gg
        [true, true, true, true, true, true, false] => Some(0),
        [false, true, true, false, false, false, false] => Some(1),
        [true, true, false, true, true, false, true] => Some(2),
        [true, true, true, true, false, false, true] => Some(3),
        [false, true, true, false, false, true, true] => Some(4),
        [true, false, true, true, false, true, true] => Some(5),
        [true, false, true, true, true, true, true] => Some(6),
        [true, true, true, false, false, false, false] => Some(7),
        [true, true, true, true, true, true, true] => Some(8),
        [true, true, true, true, false, true, true] => Some(9),
        [false, false, false, false, false, false, false] => Some(0),
        _ => None,
    }
}
