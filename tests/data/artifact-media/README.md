# Synthetic web media fixtures

`sample.png` is a 96 × 64 solid RGB image. It contains only IHDR, IDAT and IEND
chunks. `sample.mp4` is one second of FFmpeg's synthetic `testsrc2` pattern at
96 × 64, four frames per second, encoded as H.264 without audio. Neither fixture
contains a person, a photograph, a recorded voice or project data.

The video was generated with:

```sh
ffmpeg -f lavfi -i testsrc2=size=96x64:rate=4 -t 1 -an -map_metadata -1 \
  -c:v libx264 -pix_fmt yuv420p -movflags +faststart sample.mp4
```

FFmpeg is only a fixture authoring tool; the product and tests do not require it.
These are format-validation inputs, not retained artifact revisions or human
approval records.
