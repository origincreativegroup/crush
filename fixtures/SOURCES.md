# Fixture sources and provenance

The NASA-derived clips are used as factual, non-endorsement software-test material under the
[NASA media usage guidelines](https://www.nasa.gov/nasa-brand-center/images-and-media/). They contain
no identifiable interview subjects. “NASA” and NASA marks are not used to imply endorsement.

## `earth-timelapse-silent.mp4`

- Source: NASA, “Orion First Imagery Timelapse and Original”
- Catalog: `jsc2022m000270_Orion_First_Imagery_Timelapse_and_Original_221117`
- Detail page: <https://images.nasa.gov/details/jsc2022m000270_Orion_First_Imagery_Timelapse_and_Original_221117>
- Source file: <https://images-assets.nasa.gov/video/jsc2022m000270_Orion_First_Imagery_Timelapse_and_Original_221117/jsc2022m000270_Orion_First_Imagery_Timelapse_and_Original_221117~mobile.mp4>
- Transform: first 20 seconds, 640×360, 15 fps, audio removed
- SHA-256: `102dae9efb389ad98b9362cd11905efcddb6baff7fe5c3671239e6d89f465e3f`

## `goodnight-earth-vertical.mp4`

- Source: NASA, “Goodnight Earth”
- Catalog: `art002m1200962241_Goodnight-Earth_mp4`
- Detail page: <https://images.nasa.gov/details/art002m1200962241_Goodnight-Earth_mp4>
- Source file: <https://images-assets.nasa.gov/video/art002m1200962241_Goodnight-Earth_mp4/art002m1200962241_Goodnight-Earth_mp4~mobile.mp4>
- Transform: first 20 seconds, 272×480 vertical frame, 15 fps, AAC audio retained
- SHA-256: `9480db419ebbc5b6efbf2fb080eb89e725bb24c3f549c9fc87414501820f74fa`

## `rocket-launch.mp4`

- Source: NASA Kennedy Space Center, “Rocket Lab PREFIRE 1 Launch”
- Catalog: `KSC-20240525-MH-RKL01-0001-Rocket_Lab_PREFIRE_1_Launch_1080p-M6988`
- Detail page: <https://images.nasa.gov/details/KSC-20240525-MH-RKL01-0001-Rocket_Lab_PREFIRE_1_Launch_1080p-M6988>
- Source file: <https://images-assets.nasa.gov/video/KSC-20240525-MH-RKL01-0001-Rocket_Lab_PREFIRE_1_Launch_1080p-M6988/KSC-20240525-MH-RKL01-0001-Rocket_Lab_PREFIRE_1_Launch_1080p-M6988~mobile.mp4>
- Transform: source seconds 10–30, 640×360, 15 fps, AAC audio retained
- SHA-256: `eab40ed8416eead37a5e0851d0b11c69a11d0c5cda3b7c0036b98e600dfdc002`

## `synthetic-speech.mp4`

- Source: generated locally; no third-party visual or recording
- Visual: FFmpeg `testsrc2`, 640×360, 30 fps, 12 seconds
- Voice: macOS `say`, Samantha voice
- Script: “A camera follows a small red boat across the quiet blue water. The lighthouse is visible
  beyond the rocky shore.”
- SHA-256: `f9945a1e4298c50d5693de69e4657343f302666f1a4e6d6c7ff704e2432a065b`
