# Sidecar sources and build record

## FFmpeg 9.0.1 (arm64 macOS)

Source archive:

- URL: `https://ffmpeg.org/releases/ffmpeg-9.0.1.tar.xz`
- SHA-256: `cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635`

Configuration used with Apple clang 17.0.0:

```text
--arch=arm64
--target-os=darwin
--cc=clang
--disable-shared
--enable-static
--disable-doc
--disable-debug
--disable-ffplay
--disable-autodetect
--enable-videotoolbox
--enable-audiotoolbox
--enable-pic
```

This is an LGPL 2.1-or-later build. Neither `--enable-gpl` nor `--enable-nonfree` is present, and
the FFmpeg libraries are linked statically into the executables. `otool -L` reports only Apple
system frameworks and `/usr/lib/libSystem.B.dylib`; there are no Homebrew or FFmpeg dylib
dependencies. Static-linking LGPL compliance obligations still apply to distribution and must be
handled by the later packaging/license task.

| File | Bytes | SHA-256 |
|---|---:|---|
| `sidecars/ffmpeg` | 21,952,040 | `73a2114706389cad8a87890bb77b0dbe2031647acf25d6dcf48baf32fae29d0d` |
| `sidecars/ffprobe` | 21,760,424 | `da8681f30f30c6b344a2e40899b5c5669d0e501712c1867305a5027b3d6380d8` |

The binaries are intentionally ignored by Git. The production sidecar task will automate fetching
or reproducing these artifacts and will carry the notices and relinkable materials required by the
chosen LGPL distribution method.
