# Synthetic playable preview fixture

`video.mxf` is one second of 256×144 / 24 fps DNxHR LB test bars (203,833 bytes).
`audio.mxf` is one second of mono 48 kHz PCM s16le / 440 Hz tone (102,969 bytes).
`audio-right.mxf` adds a second mono 880 Hz tone (102,969 bytes).
Total: 409,771 bytes. Generated locally from FFmpeg's synthetic sources; none of
this essence comes from sample/customer media. No personal metadata is supplied.

Reproduce with a development ffmpeg (never bundled from the development machine):

```sh
ffmpeg -f lavfi -i testsrc2=size=256x144:rate=24 -t 1 -c:v dnxhd -profile:v dnxhr_lb -pix_fmt yuv422p -threads 2 -timecode 00:00:00:00 -f mxf_opatom video.mxf
ffmpeg -f lavfi -i sine=frequency=440:sample_rate=48000 -t 1 -c:a pcm_s16le -f mxf_opatom audio.mxf
ffmpeg -f lavfi -i sine=frequency=880:sample_rate=48000 -t 1 -c:a pcm_s16le -f mxf_opatom audio-right.mxf
```

`cargo test -p mobboss-core --test bundled_previews` uses `MOBBOSS_FFMPEG` and the
production thumbnail/proxy builders. It skips without ffmpeg unless
`MOBBOSS_REQUIRE_CODEC_SMOKE=1` is set (the release workflow sets it). On Mac and
Windows it requires a JPEG plus H.264/AAC faststart MP4 with two-channel audio
from both mono duplication and two-track merging;
Linux exercises its normal WebM proxy. It does not substitute software encoders
when native hardware encoding fails. The test joins the independent material
UIDs in memory to form one clip, without editing these committed inputs.
