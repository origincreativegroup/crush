# CLIP image preprocessing

Task 7 fixes the production contract at `[1, 3, 224, 224]` contiguous `f32` in NCHW order:

1. decode to RGB8;
2. resize the shorter side to 224, using Python's ties-to-even dimension rounding;
3. center-crop 224×224 with integer division;
4. divide each channel by 255 and apply the pinned CLIP mean and standard deviation;
5. transpose RGB HWC values to CHW.

## Resize filter investigation

The task draft proposed `image::imageops::FilterType::CatmullRom` as the closest approximation to
Pillow BICUBIC and required trying Lanczos3 if it missed the `1e-3` tensor tolerance. Neither generic
filter matches the committed answer key:

| Rust filter | Maximum normalized tensor difference on the first golden |
|---|---:|
| Catmull–Rom | 0.042660236 |
| Lanczos3 | 0.10505438 |

The final implementation follows [Pillow's `Resample.c`](https://github.com/python-pillow/Pillow/blob/main/src/libImaging/Resample.c)
behavior: scale-aware bicubic support with `a = -0.5`, normalized double-precision coefficients,
conversion to 22-bit fixed point, and an 8-bit rounded/clipped intermediate between the horizontal
and vertical passes. Pillow identifies its source license as
[MIT-CMU](https://github.com/python-pillow/Pillow/blob/main/LICENSE); attribution is also recorded in
`THIRD_PARTY.md`.

All four committed lossless PPM frames match all 150,528 reference values exactly:

| Fixture | Maximum absolute difference |
|---|---:|
| earth-timelapse-silent | 0 |
| goodnight-earth-vertical | 0 |
| rocket-launch | 0 |
| synthetic-speech | 0 |

The fixtures use lossless PPM because the pinned LGPL FFmpeg sidecar has no PNG encoder. The Rust
decoder enables and tests PNG, JPEG, and PNM inputs.

## JPEG decoder observation

A temporary quality-95, 4:4:4 JPEG made from `synthetic-speech.frame.ppm` was decoded and processed
independently by Pillow and Rust `image` 0.25.10. The end-to-end tensors differed at 982 of 150,528
values at the `1e-3` comparison threshold, with maximum absolute difference `0.028440237` (equivalent
to two blue-channel 8-bit levels after CLIP normalization). This is the expected JPEG decoder-path
variation noted by the task; lossless fixtures remain the correctness gate and the tolerance was not
loosened.

## Human review command

From the repository root on the target Mac:

```sh
cargo test -p crush-stage-embed --test preprocess_golden -- --nocapture
```

The output must list all four fixture files with `max_abs_diff=0` and finish with two passing tests.
