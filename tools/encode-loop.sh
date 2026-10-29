#!/usr/bin/env bash
# Encode a lossless reel master (agentfx-render --lossless x.mkv) as a seamless web loop.
# usage: encode-loop.sh master.mkv outdir basename
#   -> outdir/basename.mp4 (H.264 yuv420p, faststart), outdir/basename.webm (VP9), basename-poster.webp
# Loop-friendly: exactly the master's N
# frames, CFR 30, no B-frames (no composition offset / edit list), one keyframe at frame 0,
# timestamps from 0. Size is kept (no scaling).
set -euo pipefail
in=$1; out=$2; base=$3
FF=${FFMPEG:-ffmpeg}
CRF_X264=${CRF_X264:-18}
CRF_VP9=${CRF_VP9:-32}
mkdir -p "$out"
N=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$in")
G=$((N > 0 ? N : 300))
nice -n 10 $FF -hide_banner -loglevel error -y -fflags +genpts -i "$in" -vf "fps=30,format=yuv420p" -an -frames:v "$N" \
  -c:v libx264 -preset veryslow -tune animation -crf "$CRF_X264" -profile:v high \
  -bf 0 -g "$G" -keyint_min "$G" -x264-params open-gop=0:scenecut=0 -fps_mode cfr \
  -use_editlist 0 -avoid_negative_ts make_zero -movflags +faststart "$out/$base.mp4"
for pass in 1 2; do
  dst=/dev/null; fmt=(-f null)
  [ $pass = 2 ] && { dst="$out/$base.webm"; fmt=(); }
  nice -n 10 $FF -hide_banner -loglevel error -y -fflags +genpts -i "$in" -vf "fps=30,format=yuv420p" -an -frames:v "$N" -fps_mode cfr \
    -c:v libvpx-vp9 -b:v 0 -crf "$CRF_VP9" -row-mt 1 -deadline good -cpu-used 2 -g "$G" -auto-alt-ref 0 -lag-in-frames 0 \
    -pass $pass -passlogfile "$out/.vp9-$base" "${fmt[@]}" "$dst"
done
rm -f "$out"/.vp9-"$base"*
nice -n 10 $FF -hide_banner -loglevel error -y -i "$in" -frames:v 1 -c:v libwebp -quality 88 "$out/$base-poster.webp"
echo "$N frames"; ls -l "$out/$base.mp4" "$out/$base.webm" "$out/$base-poster.webp"
