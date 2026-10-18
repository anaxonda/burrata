`opus-fixture.ogg` is a generated 4-second, mono Ogg Opus signal used only by
the feature-gated native decoder tests. It has 440 Hz, 880 Hz, and 1320 Hz
segments so seek tests can verify the decoded time range rather than merely
checking that samples exist.

It can be regenerated with:

```sh
ffmpeg -hide_banner -loglevel error -f lavfi -i "aevalsrc='if(lt(t,1),0.25*sin(2*PI*440*t),if(lt(t,2),0.25*sin(2*PI*880*t),0.25*sin(2*PI*1320*t)))':s=48000:d=4:c=mono" -c:a libopus -b:a 96k -vbr off -y opus-fixture.ogg
```
