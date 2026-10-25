#![allow(clippy::unreadable_literal, clippy::excessive_precision)]

pub const BIAS: f32 = 0.590548;

pub const WEIGHTS: [f32; 12] = [
    0.445940,
    -0.301307,
    0.708033,
    1.076710,
    -3.493101,
    -3.627484,
    2.972956,
    -1.137400,
    -0.045269,
    0.264661,
    -0.428674,
    -0.811834,
];

#[cfg(test)]
pub const MEASURED_VETO: f32 = 0.40;

#[cfg(test)]
pub const PIN_FEATURES: [f32; 12] = [1.5898, 0.9444, 0.9444, 0.9996, 0.0000, 1.0000, 1.0683, 0.0000, 1.5898, 1.5891, 0.9996, 0.9440];
#[cfg(test)]
pub const PIN_PROBABILITY: f32 = 0.811861;
