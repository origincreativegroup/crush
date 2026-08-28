# Task 016 validation

Validation ran on the Apple Silicon development Mac on 2026-08-28 using the same `Pipeline`,
SQLite store, CLIP model, and bundled LGPL FFmpeg/FFprobe sidecars used by the packaged app.

The representative packaged-pipeline run indexed four stills (JPEG, PNG, TIFF, HEIC) and four
production-video cases (MOV/ProRes 10-bit, M4V/H.264, MXF/DNxHD, MOV/HEVC). It completed with zero
failures in 11,084 ms and recorded a process peak resident set of 985,481,216 bytes. HEVC alone
selected a working proxy; edit-friendly ProRes and DNxHD and ordinary H.264 stayed on the direct
path.

The source-fidelity fixture additionally verified all eight EXIF orientation mappings, an actual
orientation-6 JPEG, deterministic derivative hashes, source hashes unchanged before/after decode,
ICC profile retention when exposed, and a real HEIC ImageIO full render. A corrupt `.cr3` fixture
returned a decoder-specific error rather than falling back to an embedded preview.

The runtime capability matrix covers DNG, CR2/CR3, NEF, ARW, ORF, RAF, and RW2 using captured
ImageIO evidence. Those formats remain explicitly camera/OS conditional: Crush only accepts a file
after the installed ImageIO advertises its extension and completes a full render. The test does not
claim that one synthetic RAW sample proves every vendor variant.

Machine-readable evidence is in `fixtures/source-formats/validation-report-task016.json`; the
support contract and captured decoder capabilities sit beside it.
