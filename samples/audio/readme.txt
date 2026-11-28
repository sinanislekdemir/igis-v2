igis-v2 audio samples
=====================

One short public-domain classical excerpt, stored as uncompressed PCM WAV so
the guest needs no decoder: 44.1 kHz, 16-bit, stereo, little-endian.

  chopin-nocturne-21.wav   Chopin, Nocturne No. 21 in C minor, B. 108.
                           Diana Hughes. 0:00.25-0:10.25 of the source. Public domain.

Source: Wikimedia Commons; the file page carries the complete recording and
the license record:

  https://commons.wikimedia.org/wiki/File:Chopin_-_Nocturne_No._21_in_C_minor,_B._108_(Diana_Hughes).flac

The excerpt was converted from the source FLAC with:

  ffmpeg -ss 0.25 -t 10 -i <source>.flac -map 0:a:0 \
    -af "afade=t=in:st=0:d=0.01,afade=t=out:st=9.7:d=0.3,loudnorm=I=-16:TP=-1.5:LRA=11" \
    -ar 44100 -ac 2 -c:a pcm_s16le chopin-nocturne-21.wav

It starts 0.25 s into the source to skip its digital-silence lead-in, and peaks
near -1.5 dBFS. The complete recording stays at the source URL. The corpus is
read-only and staged as `audio/`.

Note: `mke2fs` turns a full 1 KiB block of zeros into an ext2 hole, and the
current `ext2w` reader treats a zero block pointer as `Invalid` (spec 0031,
"Known limitation"). This excerpt deliberately has no all-zero 1 KiB block.
Keep new WAVs free of long digital silence until hole support lands.
