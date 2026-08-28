//! Exact CLIP ViT-B/32 image preprocessing contract.

use image::{imageops, DynamicImage, GenericImageView, RgbImage};

pub const IMAGE_SIZE: usize = 224;
pub const TENSOR_LEN: usize = 3 * IMAGE_SIZE * IMAGE_SIZE;
pub const MEAN: [f32; 3] = [0.48145466, 0.4578275, 0.40821073];
pub const STD: [f32; 3] = [0.26862954, 0.261_302_6, 0.275_777_1];

#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    values: Vec<f32>,
}

impl Tensor {
    pub fn zeros() -> Self {
        Self {
            values: vec![0.0; TENSOR_LEN],
        }
    }

    pub const fn shape(&self) -> [usize; 4] {
        [1, 3, IMAGE_SIZE, IMAGE_SIZE]
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }

    pub fn channel(&self, channel: usize) -> &[f32] {
        let channel_len = IMAGE_SIZE * IMAGE_SIZE;
        &self.values[channel * channel_len..(channel + 1) * channel_len]
    }
}

/// Resize the shorter side to 224, center crop, normalize, and return contiguous NCHW f32 values.
pub fn preprocess(image: &DynamicImage) -> Tensor {
    let source = image.to_rgb8();
    let (width, height) = source.dimensions();
    assert!(width > 0 && height > 0, "image dimensions must be positive");
    let shorter = width.min(height);
    let resized_width = (f64::from(width) * 224.0 / f64::from(shorter))
        .round_ties_even()
        .max(224.0) as u32;
    let resized_height = (f64::from(height) * 224.0 / f64::from(shorter))
        .round_ties_even()
        .max(224.0) as u32;
    let resized = pillow_bicubic_resize(&source, resized_width, resized_height);
    let left = (resized_width - 224) / 2;
    let top = (resized_height - 224) / 2;
    let cropped = imageops::crop_imm(&resized, left, top, 224, 224);
    let mut values = vec![0.0_f32; TENSOR_LEN];
    let channel_len = IMAGE_SIZE * IMAGE_SIZE;
    for (x, y, pixel) in cropped.pixels() {
        let pixel_index = y as usize * IMAGE_SIZE + x as usize;
        for channel in 0..3 {
            let scaled = f32::from(pixel[channel]) / 255.0;
            values[channel * channel_len + pixel_index] = (scaled - MEAN[channel]) / STD[channel];
        }
    }
    Tensor { values }
}

const PRECISION_BITS: u32 = 22;

struct Kernel {
    first: usize,
    weights: Vec<i32>,
}

/// Match Pillow's 8-bit, two-pass BICUBIC path, including fixed-point rounding between passes.
/// Algorithm reference: <https://github.com/python-pillow/Pillow/blob/main/src/libImaging/Resample.c>
fn pillow_bicubic_resize(source: &RgbImage, output_width: u32, output_height: u32) -> RgbImage {
    let (input_width, input_height) = source.dimensions();
    let horizontal = precompute_kernels(input_width as usize, output_width as usize);
    let vertical = precompute_kernels(input_height as usize, output_height as usize);
    let mut temporary = vec![0_u8; output_width as usize * input_height as usize * 3];
    let source_values = source.as_raw();
    for y in 0..input_height as usize {
        for (x, kernel) in horizontal.iter().enumerate() {
            for channel in 0..3 {
                let mut sum = 1_i64 << (PRECISION_BITS - 1);
                for (offset, &weight) in kernel.weights.iter().enumerate() {
                    let source_index =
                        (y * input_width as usize + kernel.first + offset) * 3 + channel;
                    sum += i64::from(source_values[source_index]) * i64::from(weight);
                }
                temporary[(y * output_width as usize + x) * 3 + channel] = clip8(sum);
            }
        }
    }

    let mut output = vec![0_u8; output_width as usize * output_height as usize * 3];
    for (y, kernel) in vertical.iter().enumerate() {
        for x in 0..output_width as usize {
            for channel in 0..3 {
                let mut sum = 1_i64 << (PRECISION_BITS - 1);
                for (offset, &weight) in kernel.weights.iter().enumerate() {
                    let source_index =
                        ((kernel.first + offset) * output_width as usize + x) * 3 + channel;
                    sum += i64::from(temporary[source_index]) * i64::from(weight);
                }
                output[(y * output_width as usize + x) * 3 + channel] = clip8(sum);
            }
        }
    }
    RgbImage::from_raw(output_width, output_height, output)
        .expect("resized RGB buffer has exact dimensions")
}

fn precompute_kernels(input_size: usize, output_size: usize) -> Vec<Kernel> {
    let scale = input_size as f64 / output_size as f64;
    let filter_scale = scale.max(1.0);
    let support = 2.0 * filter_scale;
    (0..output_size)
        .map(|output| {
            let center = (output as f64 + 0.5) * scale;
            let first = ((center - support + 0.5) as isize).max(0) as usize;
            let last = ((center + support + 0.5) as usize).min(input_size);
            let mut weights = (first..last)
                .map(|input| bicubic((input as f64 - center + 0.5) / filter_scale))
                .collect::<Vec<_>>();
            let total = weights.iter().sum::<f64>();
            if total != 0.0 {
                for weight in &mut weights {
                    *weight /= total;
                }
            }
            Kernel {
                first,
                weights: weights
                    .into_iter()
                    .map(|weight| (weight * f64::from(1_u32 << PRECISION_BITS)).round() as i32)
                    .collect(),
            }
        })
        .collect()
}

fn bicubic(mut value: f64) -> f64 {
    value = value.abs();
    if value < 1.0 {
        return (1.5 * value - 2.5) * value * value + 1.0;
    }
    if value < 2.0 {
        return ((-0.5 * value + 2.5) * value - 4.0) * value + 2.0;
    }
    0.0
}

fn clip8(value: i64) -> u8 {
    (value >> PRECISION_BITS).clamp(0, 255) as u8
}
