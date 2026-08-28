use crush_stage_embed::preprocess::{preprocess, IMAGE_SIZE, TENSOR_LEN};
use serde::Deserialize;
use std::{fs, path::PathBuf};

#[derive(Deserialize)]
struct Golden {
    input: String,
    tensor: Vec<f32>,
    tensor_shape: [usize; 4],
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn preprocess_golden() {
    let golden_dir = root().join("fixtures/golden");
    let mut files = fs::read_dir(&golden_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".image.json"))
        })
        .collect::<Vec<_>>();
    files.sort();
    assert!(!files.is_empty(), "no image goldens found");

    for golden_path in files {
        let golden: Golden = serde_json::from_slice(&fs::read(&golden_path).unwrap()).unwrap();
        assert_eq!(golden.tensor_shape, [1, 3, IMAGE_SIZE, IMAGE_SIZE]);
        assert_eq!(golden.tensor.len(), TENSOR_LEN);
        let image = image::open(root().join(&golden.input)).unwrap();
        let actual = preprocess(&image);
        assert_eq!(actual.shape(), golden.tensor_shape);

        let mut max_abs_diff = 0.0_f32;
        let mut mismatches = Vec::new();
        for (index, (&found, &expected)) in
            actual.values().iter().zip(golden.tensor.iter()).enumerate()
        {
            let difference = (found - expected).abs();
            max_abs_diff = max_abs_diff.max(difference);
            if difference >= 1e-3 && mismatches.len() < 10 {
                let channel_len = IMAGE_SIZE * IMAGE_SIZE;
                let channel = index / channel_len;
                let within_channel = index % channel_len;
                mismatches.push(format!(
                    "(c={channel}, y={}, x={}): found={found}, expected={expected}, diff={difference}",
                    within_channel / IMAGE_SIZE,
                    within_channel % IMAGE_SIZE
                ));
            }
        }
        assert!(
            max_abs_diff < 1e-3,
            "{} max_abs_diff={max_abs_diff}; first mismatches:\n{}",
            golden_path.display(),
            mismatches.join("\n")
        );
        println!("{} max_abs_diff={max_abs_diff}", golden_path.display());
    }
}

#[test]
fn jpeg_and_png_decode_to_the_tensor_contract() {
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;

    let source = RgbImage::from_fn(320, 180, |x, y| {
        Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
    });
    for format in [ImageFormat::Jpeg, ImageFormat::Png] {
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(source.clone())
            .write_to(&mut encoded, format)
            .unwrap();
        let decoded = image::load_from_memory_with_format(encoded.get_ref(), format).unwrap();
        let tensor = preprocess(&decoded);
        assert_eq!(tensor.shape(), [1, 3, IMAGE_SIZE, IMAGE_SIZE]);
        assert_eq!(tensor.values().len(), TENSOR_LEN);
        assert!(tensor.values().iter().all(|value| value.is_finite()));
    }
}
